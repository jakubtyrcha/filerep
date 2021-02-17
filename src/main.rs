extern crate clap;
use clap::{Arg, App};
use std::net::{SocketAddr};
use std::error::Error;
use std::io;
use futures::sink::SinkExt;
use futures::stream::StreamExt;
use std::path::Path;
use std::sync::Arc;
use notify::{Watcher, RecursiveMode, watcher};
use tokio::fs::File;
use tokio::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, TcpListener};
use tokio::sync::{Notify, RwLock, watch};
use tokio_util::codec::{Framed};

mod encdec;
use encdec::{FileChunkDecoder, FileChunkEncoder, FileChunk};

// TODO
// replace with const_format when stable
fn get_handshake_msg() -> Vec<u8> {
    Vec::from(format!("filerep v{}", 3.0).as_bytes())
}

async fn run_client(address : SocketAddr, path : &Path) -> Result<(), Box<dyn Error>> {
    let stream = TcpStream::connect(address).await?;
    stream.writable().await?;

    // handshake
    loop {
        let handshake_array = get_handshake_msg();
        let mut sent = 0;
        match stream.try_write(&handshake_array[sent..]) {
            Ok(n) => {
                sent += n;
                if sent == handshake_array.len() {
                    break;
                }
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

    let mut file = File::create(path).await.unwrap();

    // start receiving data
    stream.readable().await?;
    let mut dec = Framed::new(stream, FileChunkDecoder{});

    loop {
        match dec.next().await {
            Some(chunk) => {
                file.write(&chunk.unwrap().data[..]).await;
            },
            None => {

            }
        }
    }
}

struct Node {
    next : Arc<RwLock<Option<Node>>>,
    data : Vec<u8>,
    gen : i64
}

impl Node {
    fn new(data : Vec<u8>, gen : i64) -> Node { 
        Node { 
            next : Arc::new(RwLock::new(None)), data, gen
        } 
    }
    async fn append(&mut self, node : Node) -> Arc<RwLock<Option<Node>>> {
        let mut n = self.next.write().await;
        *n = Some(node);
        return self.next.clone();
    }
}

async fn run_server(address : SocketAddr, path : &Path) -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind(&address)
        .await
        .expect("unable to bind TCP listener");

    let notify_file_changed = Arc::new(Notify::new());
    let notify_file_changed_receiver = notify_file_changed.clone();

    let notify_change_broadcasted_tx = Arc::new(Notify::new());
    let notify_change_broadcasted_rx = notify_change_broadcasted_tx.clone();
    
    let (file_tx, file_rx) = std::sync::mpsc::channel();
    // this specifies how events are debounced (grouped) if they happen within a short interval
    let mut watcher = watcher(file_tx, Duration::from_millis(5)).unwrap();
    watcher.watch(path, RecursiveMode::NonRecursive).unwrap();

    let mut file = File::open(path).await.unwrap();
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).await?;

    // this is the file changed event loop
    tokio::spawn(async move {
        loop {
            // todo: this should make sure we progressed with other work before we start handling more notifications?
            let mut write_detected = false;
            match file_rx.recv() {
                Ok(event) => {
                    match event {
                        notify::DebouncedEvent::Write(_) => {
                            write_detected = true;
                        }
                        _ => {}
                    }
                },
                _ => {}
            }

            if write_detected {
                notify_file_changed.notify_one();
                println!("File change detected and signalled");
                notify_change_broadcasted_rx.notified().await;
            }
        }
    });

    let file_chunks_list_head = Arc::new(RwLock::new(Some(Node::new(buffer, 0))));
    let mut write_head = file_chunks_list_head.clone();
    let (list_tx, list_rx) = watch::channel(0);
    
    tokio::spawn(async move {
        let mut file = Box::pin(file);
        loop {
            notify_file_changed_receiver.notified().await;
            //let seek_res = file.as_mut().seek(SeekFrom::End(0)).await.unwrap();
            let mut buffer = Vec::new();
            file.as_mut().read_to_end(&mut buffer).await;

            println!("Read new file chunk of len {}", buffer.len());

            let next_head;
            let gen;
            {
                let mut write_guard = write_head.write().await;
                let node = write_guard.as_mut().unwrap();
                gen = node.gen + 1;
                next_head = node.append(Node::new(buffer, gen)).await;
            }
            write_head = next_head;
            list_tx.send(gen);
            notify_change_broadcasted_tx.notify_one();
        }
    });

    loop {
        let (socket, _) = listener.accept().await?;
        let mut connection_list_rx = list_rx.clone();
        let connection_list_head = file_chunks_list_head.clone();
        
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
                            break;
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

            // handshake succesful
            let mut read_head = connection_list_head;
            let mut framed = Framed::new(socket, FileChunkEncoder{});
            let mut offset = 0;

            loop {
                let mut next_head = None;
                {
                    let read_guard = read_head.read().await;
                    if let Some(node) = read_guard.as_ref() {
                        next_head = Some(node.next.clone());
                        // write to socket
                        framed.send(FileChunk{ offset : offset, data : &node.data }).await?;
                        offset = offset + node.data.len() as u64;
                    }
                }
                if let Some(next_head) = next_head {
                    read_head = next_head;
                }
                else {
                    connection_list_rx.changed().await;
                }
            }
            
        });
    }
}

#[tokio::main]
async fn main() {
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

    if(is_server) {
        run_server(socket_address, Path::new(&path)).await;
    }
    else {
        run_client(socket_address, Path::new(&path)).await;
    }
}
