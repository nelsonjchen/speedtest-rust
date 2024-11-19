#![allow(dead_code)]

use std::{
    io::Read,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tracing::info;

use reqwest::blocking::{Body, Client, Request, Response};
use reqwest::header::{HeaderValue, CONNECTION, CONTENT_TYPE, REFERER, USER_AGENT};
use reqwest::Url;

use crate::distance::EarthLocation;
use crate::error::SpeedTestError;
use crate::speedtest_config::SpeedTestConfig;
use crate::speedtest_servers_config::SpeedTestServersConfig;
use rayon::prelude::*;

const ST_USER_AGENT: &str = concat!("reqwest/speedtest-rs ", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Debug)]
pub struct SpeedTestServer {
    pub country: String,
    pub host: String,
    pub id: u32,
    pub location: EarthLocation,
    pub distance: Option<f32>,
    pub name: String,
    pub sponsor: String,
    pub url: String,
}

pub fn download_configuration() -> Result<Response, SpeedTestError> {
    info!("Downloading Configuration from speedtest.net");

    let mut _server = mockito::Server::new();

    #[cfg(not(test))]
    let url = "http://www.speedtest.net/speedtest-config.php";
    #[cfg(test)]
    let url = &format!("{}/speedtest-config.php", &_server.url());

    let client = Client::new();
    // Creating an outgoing request.
    let res = client
        .get(url)
        .header(CONNECTION, "close")
        .header(USER_AGENT, ST_USER_AGENT.to_owned())
        .send()?;
    info!("Downloaded Configuration from speedtest.net");
    Ok(res)
}

pub fn get_configuration() -> Result<SpeedTestConfig, SpeedTestError> {
    let config_body = download_configuration()?;
    info!("Parsing Configuration");
    let spt_config = SpeedTestConfig::parse(&(config_body.text()?))?;
    info!("Parsed Configuration");
    Ok(spt_config)
}

pub fn download_server_list() -> Result<Response, SpeedTestError> {
    info!("Download Server List");
    let mut _server = mockito::Server::new();

    #[cfg(not(test))]
    let url = "http://www.speedtest.net/speedtest-servers.php";
    #[cfg(test)]
    let url = &format!("{}/speedtest-servers.php", &_server.url());

    let client = Client::new();
    let server_res = client
        .get(url)
        .header(CONNECTION, "close")
        .header(USER_AGENT, ST_USER_AGENT)
        .send()?;
    info!("Downloaded Server List");
    Ok(server_res)
}

pub fn get_server_list_with_config(
    config: &SpeedTestConfig,
) -> Result<SpeedTestServersConfig, SpeedTestError> {
    let config_body = download_server_list()?;
    info!("Parsing Server List");
    let server_config_string = config_body.text()?;

    info!("Parsed Server List");
    SpeedTestServersConfig::parse_with_config(&server_config_string, config)
}

#[derive(Debug)]
pub struct SpeedTestLatencyTestResult<'a> {
    pub server: &'a SpeedTestServer,
    pub latency: Duration,
}

pub fn get_best_server_based_on_latency(
    servers: &[SpeedTestServer],
) -> Result<SpeedTestLatencyTestResult, SpeedTestError> {
    info!("Testing for fastest server");
    let client = Client::new();
    let mut fastest_server = None;
    let mut fastest_latency = Duration::new(u64::MAX, 0);
    // Return error if no servers are available.
    if servers.is_empty() {
        return Err(SpeedTestError::LatencyTestNoServerError);
    }
    'server_loop: for server in servers {
        let path = Path::new(&server.url);
        let latency_path = format!(
            "{}/latency.txt",
            path.parent()
                .ok_or(SpeedTestError::LatencyTestInvalidPath)?
                .display()
        );
        info!("Downloading: {:?}", latency_path);
        let mut latency_measurements = vec![];
        for _ in 0..3 {
            let start_time = SystemTime::now();
            let res = client
                .get(&latency_path)
                .header(CONNECTION, "close")
                .header(USER_AGENT, ST_USER_AGENT.to_owned())
                .send();
            if res.is_err() {
                // Log the error and continue to the next server.
                info!("Error: {:?}", res.err());
                continue 'server_loop;
            }
            let _ = res?.bytes()?.last();
            let latency_measurement = SystemTime::now().duration_since(start_time)?;
            info!("Sampled {} ms", latency_measurement.as_millis());
            latency_measurements.push(latency_measurement);
        }
        // Divide by the double to get the non-RTT time but the trip time.
        // NOT PING or RTT
        // https://github.com/sivel/speedtest-cli/pull/199
        let latency = latency_measurements
            .iter()
            .fold(Duration::new(0, 0), |a, &i| a + i)
            / ((latency_measurements.len() as u32) * 2);
        info!("Trip calculated to {} ms", latency.as_millis());

        if latency < fastest_latency {
            fastest_server = Some(server);
            fastest_latency = latency;
        }
    }
    info!(
        "Fastest Server @ {}ms : {fastest_server:?}",
        fastest_latency.as_millis(),
    );
    Ok(SpeedTestLatencyTestResult {
        server: fastest_server.ok_or(SpeedTestError::LatencyTestClosestError)?,
        latency: fastest_latency,
    })
}

#[derive(Debug)]
pub struct SpeedMeasurement {
    pub size: usize,
    pub duration: Duration,
}

impl SpeedMeasurement {
    pub fn kbps(&self) -> u32 {
        (self.size as u32 * 8) / self.duration.as_millis() as u32
    }

    pub fn bps_f64(&self) -> f64 {
        (self.size as f64 * 8.0) / (self.duration.as_millis() as f64 / (1000.0))
    }
}

pub fn test_download_with_progress_and_config<F>(
    server: &SpeedTestServer,
    progress_callback: F,
    config: &mut SpeedTestConfig,
) -> Result<SpeedMeasurement, SpeedTestError>
where
    F: Fn() + Send + Sync + 'static,
{
    info!("Testing Download speed");
    let root_url = Url::parse(&server.url)?;

    let mut urls = vec![];
    for size in &config.sizes.download {
        let mut download_with_size_url = root_url.clone();
        {
            let mut path_segments_mut = download_with_size_url
                .path_segments_mut()
                .map_err(|_| SpeedTestError::ServerParseError)?;
            path_segments_mut.push(&format!("random{size}x{size}.jpg"));
        }
        for _ in 0..config.counts.download {
            urls.push(download_with_size_url.clone());
        }
    }

    let _request_count = urls.len();

    let requests = urls
        .iter()
        .enumerate()
        .map(|(i, url)| {
            let mut cache_busting_url = url.clone();
            cache_busting_url.query_pairs_mut().append_pair(
                "x",
                &format!(
                    "{}.{i}",
                    SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
                ),
            );
            let mut request = Request::new(reqwest::Method::GET, url.clone());
            request.headers_mut().insert(
                reqwest::header::CACHE_CONTROL,
                HeaderValue::from_static("no-cache"),
            );
            request.headers_mut().insert(
                reqwest::header::USER_AGENT,
                HeaderValue::from_static(ST_USER_AGENT),
            );
            request.headers_mut().insert(
                reqwest::header::CONNECTION,
                HeaderValue::from_static("close"),
            );
            Ok(request)
        })
        .collect::<Result<Vec<_>, SpeedTestError>>()?;

    // TODO: Setup Ctrl-C Termination to use this "event".
    let early_termination = AtomicBool::new(false);

    // Start Timer
    let start_time = SystemTime::now();

    info!("Download Threads: {}", config.threads.download);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(config.threads.download)
        .build()?;

    info!("Total to be requested {requests:?}");

    let total_transferred_per_thread = pool.install(|| {
        requests
            .into_iter()
            // Make it sequential like the original. Ramp up the file sizes.
            .par_bridge()
            .map(|r| {
                let client = Client::new();
                // let downloaded_count = vec![];
                progress_callback();
                info!("Requesting {}", r.url());
                let mut response = client.execute(r)?;
                let mut buf = [0u8; 10240];
                let mut read_amounts = vec![];
                while (SystemTime::now().duration_since(start_time)? < config.length.upload)
                    && !early_termination.load(Ordering::Relaxed)
                {
                    let read_amount = response.read(&mut buf)?;
                    read_amounts.push(read_amount);
                    if read_amount == 0 {
                        break;
                    }
                }
                let total_transfered = read_amounts.iter().sum::<usize>();
                progress_callback();

                Ok(total_transfered)
            })
            .collect::<Result<Vec<_>, SpeedTestError>>()
    });

    let total_transferred: usize = total_transferred_per_thread?.iter().sum();

    let end_time = SystemTime::now();

    let measurement = SpeedMeasurement {
        size: total_transferred,
        duration: end_time.duration_since(start_time)?,
    };

    if measurement.bps_f64() > 100000.0 {
        config.threads.upload = 8
    }

    Ok(measurement)
}

#[derive(Debug)]
pub struct SpeedTestUploadRequest {
    pub request: Request,
    pub size: usize,
}

pub fn test_upload_with_progress_and_config<F>(
    server: &SpeedTestServer,
    progress_callback: F,
    config: &SpeedTestConfig,
) -> Result<SpeedMeasurement, SpeedTestError>
where
    F: Fn() + Send + Sync + 'static,
{
    info!("Testing Upload speed");

    let mut sizes = vec![];
    for &size in &config.sizes.upload {
        for _ in 0..config.counts.upload {
            sizes.push(size)
        }
    }

    let best_url = Url::parse(&server.url)?;

    let request_count = config.upload_max;

    let requests = sizes
        .into_iter()
        .map(|size| {
            let content_iter = b"content1="
                .iter()
                .chain(b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ".iter().cycle())
                .take(size);
            let content_iter_read = iter_read::IterRead::new(content_iter);
            let body = Body::sized(content_iter_read, size as u64);
            let mut request = Request::new(reqwest::Method::POST, best_url.clone());
            request.headers_mut().insert(
                reqwest::header::USER_AGENT,
                HeaderValue::from_static(ST_USER_AGENT),
            );
            request.headers_mut().insert(
                reqwest::header::CONNECTION,
                HeaderValue::from_static("close"),
            );
            *request.body_mut() = Some(body);
            Ok(SpeedTestUploadRequest { request, size })
        })
        .collect::<Result<Vec<_>, SpeedTestError>>()?;
    // TODO: Setup Ctrl-C Termination to use this "event".
    let early_termination = AtomicBool::new(false);

    // Start Timer
    let start_time = SystemTime::now();

    info!("Upload Threads: {}", config.threads.upload);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(config.threads.upload)
        .build()?;

    info!("Total to be requested {:?}", requests.len());
    let total_transferred_per_thread = pool.install(|| {
        requests
            .into_iter()
            .take(request_count)
            // Make it sequential like the original. Ramp up the file sizes.
            .par_bridge()
            .map(|r| {
                progress_callback();

                if (SystemTime::now().duration_since(start_time)? < config.length.upload)
                    && !early_termination.load(Ordering::Relaxed)
                {
                    let client = Client::new();
                    info!("Requesting {}", r.request.url());
                    let response = client.execute(r.request);
                    if response.is_err() {
                        return Ok(r.size);
                    };
                } else {
                    return Ok(0);
                }
                progress_callback();

                Ok(r.size)
            })
            .collect::<Result<Vec<_>, SpeedTestError>>()
    });

    let total_transferred: usize = total_transferred_per_thread?.iter().sum();

    let end_time = SystemTime::now();

    let measurement = SpeedMeasurement {
        size: total_transferred,
        duration: end_time.duration_since(start_time)?,
    };

    Ok(measurement)
}

#[derive(Debug)]
pub struct SpeedTestResult<'a, 'b, 'c> {
    pub download_measurement: Option<&'a SpeedMeasurement>,
    pub upload_measurement: Option<&'b SpeedMeasurement>,
    pub server: &'c SpeedTestServer,
    pub latency_measurement: &'c SpeedTestLatencyTestResult<'c>,
}

impl SpeedTestResult<'_, '_, '_> {
    pub fn hash(&self) -> String {
        let hashed_str = format!(
            "{}-{}-{}-{}",
            self.latency_measurement.latency.as_millis(),
            self.upload_measurement.map_or(0, |x| x.kbps()),
            self.download_measurement.map_or(0, |x| x.kbps()),
            "297aae72"
        );

        format!("{:x}", md5::compute(hashed_str))
    }
}

pub fn get_share_url(speedtest_result: &SpeedTestResult) -> Result<String, SpeedTestError> {
    info!("Generating share URL");

    let download = speedtest_result
        .download_measurement
        .map_or(0, |x| x.kbps());
    info!("Download parameter is {download:?}");
    let upload = speedtest_result.upload_measurement.map_or(0, |x| x.kbps());
    info!("Upload parameter is {upload:?}");
    let server = speedtest_result.server.id;
    info!("Server parameter is {server:?}");
    let ping = speedtest_result.latency_measurement.latency;
    info!("Ping parameter is {ping:?}");

    let pairs = [
        ("download", download.to_string()),
        ("ping", ping.as_millis().to_string()),
        ("upload", upload.to_string()),
        ("promo", String::new()),
        ("startmode", "pingselect".to_string()),
        ("recommendedserverid", format!("{server}")),
        ("accuracy", "1".to_string()),
        ("serverid", format!("{server}")),
        ("hash", speedtest_result.hash()),
    ];

    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs.iter())
        .finish();

    info!("Share Body Request: {body:?}");

    let client = Client::new();
    let res = client
        .post("http://www.speedtest.net/api/api.php")
        .header(CONNECTION, "close")
        .header(REFERER, "http://c.speedtest.net/flash/speedtest.swf")
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(body)
        .send();
    let encode_return = res?.text()?;
    let response_id = parse_share_request_response_id(encode_return.as_bytes())?;
    Ok(format!("http://www.speedtest.net/result/{response_id}.png"))
}

pub fn parse_share_request_response_id(input: &[u8]) -> Result<String, SpeedTestError> {
    url::form_urlencoded::parse(input)
        .find(|pair| pair.0 == "resultid")
        .map_or_else(
            || Err(SpeedTestError::ParseShareUrlError),
            |pair| Ok(pair.1.into_owned()),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_share_request_response_id() {
        let resp = "resultid=4932415710&date=12%2F21%2F2015&time=5%3A10+AM&rating=0".as_bytes();
        assert_eq!(parse_share_request_response_id(resp).unwrap(), "4932415710");
    }

    #[test]
    fn test_share_url_hash() {
        let download_measurement = SpeedMeasurement {
            size: (6096 * 100) as usize,
            duration: Duration::new(1, 0),
        };
        println!("Download: {:?}", download_measurement);
        let upload_measurement = SpeedMeasurement {
            size: (1861 * 100) as usize,
            duration: Duration::new(1, 0),
        };
        println!("Upload: {:?}", upload_measurement);
        let server = SpeedTestServer {
            country: "".to_owned(),
            host: "".to_owned(),
            id: 5116,
            location: EarthLocation {
                latitude: 0.0,
                longitude: 0.0,
            },
            distance: None,
            name: "".to_owned(),
            sponsor: "".to_owned(),
            url: "".to_owned(),
        };
        println!("Server: {server:?}");
        let latency_measurement = SpeedTestLatencyTestResult {
            server: &server,
            latency: Duration::from_millis(26),
        };
        println!("Latency: {latency_measurement:?}");
        let request = SpeedTestResult {
            download_measurement: Some(&download_measurement),
            upload_measurement: Some(&upload_measurement),
            server: &server,
            latency_measurement: &latency_measurement,
        };
        assert_eq!(request.hash(), "f10eb3dd8d3c38a221e823d859680045");
    }

    #[test]
    fn test_construct_share_form() {}

    #[test]
    fn test_get_configuration() {
        let mut server = mockito::Server::new();

        let _m = server
            .mock("GET", "/speedtest-config.php")
            .with_status(200)
            .with_body_from_file("tests/config/stripped-config.php.xml")
            .create();
        let _config = get_configuration();
    }

    #[test]
    fn test_get_server_list_with_config() {
        let mut server = mockito::Server::new();

        let _m = server
            .mock("GET", "/speedtest-config.php")
            .with_status(200)
            .with_body_from_file("tests/config/servers-static.php.xml")
            .create();

        let _server_list_config = get_server_list_with_config(&SpeedTestConfig::default());
    }
}
