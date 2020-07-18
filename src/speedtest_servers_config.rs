use crate::distance::{self, EarthLocation};
use crate::{error::Error, speedtest::SpeedTestServer, speedtest_config::SpeedTestConfig};
use std::{cmp::Ordering::Less, io::Read};
use xml::reader::XmlEvent::StartElement;
use xml::EventReader;

pub struct SpeedTestServersConfig {
    pub servers: Vec<SpeedTestServer>,
}

impl SpeedTestServersConfig {
    pub fn new<R: Read>(parser: EventReader<R>) -> Result<SpeedTestServersConfig, Error> {
        SpeedTestServersConfig::with_config(parser, None)
    }

    pub fn with_config<R: Read>(
        parser: EventReader<R>,
        config: Option<&SpeedTestConfig>,
    ) -> Result<SpeedTestServersConfig, Error> {
        let mut servers: Vec<SpeedTestServer> = Vec::new();

        for event in parser {
            if let Ok(StartElement {
                ref name,
                ref attributes,
                ..
            }) = event
            {
                if name.local_name == "server" {
                    let mut country: Option<String> = None;
                    let mut host: Option<String> = None;
                    let mut id: Option<u32> = None;
                    let mut lat: Option<f32> = None;
                    let mut lon: Option<f32> = None;
                    let mut name: Option<String> = None;
                    let mut sponsor: Option<String> = None;
                    let mut url: Option<String> = None;
                    for attribute in attributes {
                        match attribute.name.local_name.as_ref() {
                            "country" => {
                                country = Some(attribute.value.clone());
                            }
                            "host" => {
                                host = Some(attribute.value.clone());
                            }
                            "id" => id = attribute.value.parse::<u32>().ok(),
                            "lat" => lat = attribute.value.parse::<f32>().ok(),
                            "lon" => lon = attribute.value.parse::<f32>().ok(),
                            "name" => {
                                name = Some(attribute.value.clone());
                            }
                            "sponsor" => {
                                sponsor = Some(attribute.value.clone());
                            }
                            "url" => {
                                url = Some(attribute.value.clone());
                            }
                            _ => {}
                        }
                    }
                    if let (
                        Some(country),
                        Some(host),
                        Some(id),
                        Some(lat),
                        Some(lon),
                        Some(name),
                        Some(sponsor),
                        Some(url),
                    ) = (country, host, id, lat, lon, name, sponsor, url)
                    {
                        let location = EarthLocation {
                            latitude: lat,
                            longitude: lon,
                        };
                        let distance = config
                            .map(|config| distance::compute_distance(&config.location, &location));
                        let server = SpeedTestServer {
                            country,
                            host,
                            id,
                            location,
                            distance,
                            name,
                            sponsor,
                            url,
                        };
                        servers.push(server);
                    }
                }
            }
        }
        Ok(SpeedTestServersConfig { servers })
    }

    pub fn servers_sorted_by_distance(&self, config: &SpeedTestConfig) -> Vec<SpeedTestServer> {
        let location = &config.location;
        let mut sorted_servers = self.servers.clone();
        sorted_servers.sort_by(|a, b| {
            let a_distance = distance::compute_distance(&location, &a.location);
            let b_distance = distance::compute_distance(&location, &b.location);
            a_distance.partial_cmp(&b_distance).unwrap_or(Less)
        });
        sorted_servers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_speedtest_servers_xml() {
        let parser = EventReader::new(include_bytes!(
            "../tests/confi\
             g/stripped-ser\
             vers-static.\
             php.xml"
        ) as &[u8]);
        let spt_server_config = SpeedTestServersConfig::new(parser).unwrap();
        assert!(spt_server_config.servers.len() > 5);
        let server = spt_server_config.servers.get(1).unwrap();
        assert!(!server.url.is_empty());
        assert!(!server.country.is_empty());
    }

    #[test]
    fn test_fastest_server() {
        use crate::speedtest_config::*;

        let spt_config = SpeedTestConfig {
            client: SpeedTestClientConfig {
                ip: "127.0.0.1".parse().unwrap(),
                isp: "xxxfinity".to_string(),
            },
            ignore_servers: vec![],
            sizes: SpeedTestSizeConfig::default(),
            counts: SpeedTestCountsConfig::default(),
            threads: SpeedTestThreadsConfig::default(),
            length: SpeedTestLengthConfig::default(),
            upload_max: 0,
            location: EarthLocation {
                latitude: 32.9954,
                longitude: -117.0753,
            },
        };
        let parser = EventReader::new(include_bytes!(
            "../tests/confi\
             g/geo-test-ser\
             vers-static.\
             php.xml"
        ) as &[u8]);
        let config = SpeedTestServersConfig::new(parser).unwrap();
        let closest_server = &config.servers_sorted_by_distance(&spt_config)[0];
        assert_eq!("Los Angeles, CA", closest_server.name);
    }
}
