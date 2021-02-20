extern crate clap;
use clap::{Arg, App};
use std::net::{SocketAddr};
use std::error::Error;

use filerep::{run_server, run_client};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let matches = App::new("filerep")
        .arg(Arg::with_name("server").long("server").takes_value(false))
        .arg(Arg::with_name("port").required(true))
        .arg(Arg::with_name("path").required(true))
        .arg(Arg::with_name("address").required_unless("server"))
        .get_matches();

    let port = matches.value_of("port").unwrap().parse::<u16>().unwrap();
    let path : String = String::from(matches.value_of("path").unwrap());
    let is_server = matches.is_present("server");
    let address = if is_server { String::from("127.0.0.1") } else { String::from(matches.value_of("address").unwrap()) };
    let socket_address = SocketAddr::new(address.parse().unwrap(), port);

    if is_server {
        run_server(socket_address, path).await?;
    }
    else {
        run_client(socket_address, path).await?;
    }

    Ok(())
}
