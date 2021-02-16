use tokio::net::{TcpStream, TcpListener};
use std::error::Error;
use std::io;
use std::env;

// TODO
// replace with const_format when stable
fn get_handshake_msg() -> Vec<u8> {
    Vec::from(format!("filerep v{}", 3.0).as_bytes())
}

async fn run_client() -> Result<(), Box<dyn Error>> {
    let stream = TcpStream::connect("127.0.0.1:12345").await?;
    stream.writable().await?;

    loop {
        let h = get_handshake_msg();
        match stream.try_write(&h[..]) {
            Ok(n) => {
                println!("{}", n);
                break;
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                continue;
            }
            Err(e) => {
                return Err(e.into());
            }
        }
    }

    Ok(())
}

async fn run_server() -> Result<(), Box<dyn Error>> {
    let addr = "127.0.0.1:12345";
    let mut listener = TcpListener::bind(&addr)
        .await
        .expect("unable to bind TCP listener");

    loop {
        let (socket, _) = listener.accept().await?;
        
        tokio::spawn(async move {
            socket.readable().await?;

            let mut buf = [0; 4096];
            loop {
                match socket.try_read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let h = get_handshake_msg();
                        if get_handshake_msg() == buf[..h.len()] {
                            println!("handshake_successful");
                        }
                        else {
                            println!("handshake_fail {:?} {:?}", get_handshake_msg(), buf);
                            // does it close the connection?
                            return Ok::<(), io::Error>(());
                        }
                    }
                    Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                        continue;
                    }
                    Err(e) => {
                        return Err(e.into());
                    }
                }
            }
            Ok(())
        });
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();
    
    if args.len() == 1 {
        run_server().await;
    }
    else {
        run_client().await;
    }
}
