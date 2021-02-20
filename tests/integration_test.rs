use filerep;
use std::net::SocketAddr;
use std::fs;
use std::fs::File;
use std::io::Write;
use tokio::time::{sleep, Duration};

#[tokio::test]
async fn test_multiple_connections() {
    if let Err(e) = fs::create_dir("tmp") {
        if e.kind() != std::io::ErrorKind::AlreadyExists {
            panic!("can't create tmp dir");
        }
    }

    let mut v = Vec::new();
    let mut f = File::create("tmp/test").unwrap();
    let NUM_CLIENTS = 6;

    tokio::spawn(filerep::run_server(SocketAddr::new("127.0.0.1".parse().unwrap(), 12345), String::from("tmp/test")));
    for i in 0..NUM_CLIENTS {
        tokio::spawn(filerep::run_client(SocketAddr::new("127.0.0.1".parse().unwrap(), 12345), format!("tmp/out{}", i)));
    }

    v.extend_from_slice(b"Hello, world! > ");
    f.write_all(&v[..]);
    f.flush();

    let mut offset = v.len();

    for i in 1..64 {
        for _ in 0..i {
            v.push('a' as u8)
        }

        f.write_all(&v[offset..]);
        f.flush();

        offset += i;
    }
    
    // TODO: signal tasks to end
    sleep(Duration::from_millis(100)).await;

    for i in 0..NUM_CLIENTS {
        assert_eq!(fs::read(format!("tmp/out{}", i)).unwrap(), &v[..]);
    }

   fs::remove_dir_all("tmp");
}
