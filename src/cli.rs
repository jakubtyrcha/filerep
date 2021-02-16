extern crate clap;
use clap::{Arg, App};
extern crate tokio;
use tokio::fs::File;
use tokio::runtime::Builder;
use std::io::{SeekFrom};
use tokio::io::AsyncSeekExt;
use tokio::time;
use std::time::Duration;
extern crate futures;
use futures::executor::block_on;

fn main() {
    let matches = App::new("My Super Program")
        .arg(Arg::with_name("server").long("server").takes_value(false))
        .arg(Arg::with_name("port").required(true))
        .arg(Arg::with_name("path").required(true))
        .arg(Arg::with_name("address").required_unless_one(&["server"]))
        .get_matches();
                          
    let is_server = matches.is_present("server");
    let port = matches.value_of("port").unwrap().parse::<i32>().unwrap();
    let path : String = String::from(matches.value_of("path").unwrap());

    if is_server {
        println!("server > port: {}, path: {}", port, path);

        
    }
    else {
        let address = matches.value_of("address").unwrap();
        println!("client > port: {}, path: {}, address: {}", port, path, address);
    }
}
