extern crate clap;
use clap::{Arg, App};
use std::net::{SocketAddr};
use std::error::Error;
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

mod chunk;
use chunk::{FileChunkDecoder, FileChunkEncoder, FileChunk};

mod handshake;
use handshake::{HandshakeDecoder, HandshakeEncoder};

async fn run_client(address : SocketAddr, path : &Path) -> Result<(), Box<dyn Error>> {
    let stream = TcpStream::connect(address).await?;
    stream.writable().await?;

    // handshake
    let mut handshake_enc = Framed::new(stream, HandshakeEncoder{});
    handshake_enc.send(()).await?;

    let stream = handshake_enc.into_inner();

    let mut file = File::create(path).await?;

    // start receiving data
    stream.readable().await?;
    let mut dec = Framed::new(stream, FileChunkDecoder{});

    loop {
        match dec.next().await {
            Some(chunk) => {
                file.write(&chunk.unwrap().data[..]).await?;
            },
            None => {
                return Ok(());
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
    
    let (file_tx, file_rx) = std::sync::mpsc::channel();
    // this specifies how events are debounced (grouped) if they happen within a short interval
    let mut watcher = watcher(file_tx, Duration::from_millis(5)).unwrap();
    if let Err(e) = watcher.watch(path, RecursiveMode::NonRecursive) {
        println!("Can't watch {}: {}", path.display(), e);
        return Err(e.into());
    }

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
                tokio::task::yield_now().await;
            }
        }
    });

    let file_chunks_list_head = Arc::new(RwLock::new(Some(Node::new(buffer, 0))));
    let mut write_head = file_chunks_list_head.clone();
    let (list_tx, list_rx) = watch::channel(0);
    
    tokio::spawn(
        async move {
        let mut file = Box::pin(file);
        loop {
            notify_file_changed_receiver.notified().await;
            let mut buffer = Vec::new();
            file.as_mut().read_to_end(&mut buffer).await?;

            if buffer.len() > 0 {
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
                list_tx.send(gen)?;
            }

        }
        // this is here because the compiler can't infer the return type
        #[allow(unreachable_code)]
        Ok::<_, Box<dyn Error + Send + Sync>>(())
    });

    loop {
        let (socket, _) = listener.accept().await?;
        let mut connection_list_rx = list_rx.clone();
        let connection_list_head = file_chunks_list_head.clone();
        
        tokio::spawn(async move {
            socket.readable().await?;

            let mut handshake_dec = Framed::new(socket, HandshakeDecoder{});
            if let Some(Err(e)) = handshake_dec.next().await {
                return Err(e.into());
            }

            let socket = handshake_dec.into_inner();

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
                    connection_list_rx.changed().await?;
                }
            }
            // this is here because the compiler can't infer the return type
            #[allow(unreachable_code)]
            Ok::<_, Box<dyn Error + Send + Sync>>(())
        });
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
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
        run_server(socket_address, Path::new(&path)).await?;
    }
    else {
        run_client(socket_address, Path::new(&path)).await?;
    }

    Ok(())
}
