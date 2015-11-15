extern crate speedtest_rs;

#[macro_use]
extern crate log;
extern crate env_logger;

#[macro_use]
extern crate clap;

use clap::{App, Arg};
// use speedtest_rs::speedtest::run_speedtest;
use speedtest_rs::speedtest;

#[allow(dead_code)]
fn main() {
    env_logger::init().unwrap();
    let matches = App::new("speedtest-rs")
                      .version(&crate_version!()[..])
                      .about("Command line interface for testing internet bandwidth using \
                              speedtest.net.")
                      .arg(Arg::with_name("list")
                               .help("Display a list of speedtest.net servers sorted by distance"))
                      .get_matches();

    println!("Retrieving speedtest.net configuration...", );
    let config = speedtest::get_configuration().unwrap();
    println!("Retrieving speedtest.net server list...", );
    let server_list = speedtest::get_server_list_with_config(Some(&config)).unwrap();
    let server_list_sorted = server_list.servers_sorted_by_distance(&config);

    if matches.is_present("list") {
        for server in server_list_sorted {
            println!("{:4}) {} ({}, {}) [{:.2} km]",
             server.id,
             server.sponsor,
             server.name,
             server.country,
             server.distance.unwrap(),
         );
        }
        return;
    }
    println!("Testing from {} ({})...", config.isp, config.ip);
}
