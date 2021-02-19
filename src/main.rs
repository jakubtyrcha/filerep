extern crate clap;
use clap::{Arg, App};
use std::net::{SocketAddr};
use std::error::Error;
use futures::sink::SinkExt;
use futures::stream::StreamExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::TryRecvError;
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

/*
Client sends the handshake and receives the file chunks
*/
async fn run_client(address : SocketAddr, path : &Path) -> Result<(), Box<dyn Error>> {
    let stream = TcpStream::connect(address).await?;
    stream.writable().await?;

    // `Framed` allows to send structured data, wrap the socket and send one handshake element
    let mut handshake_enc = Framed::new(stream, HandshakeEncoder{});
    handshake_enc.send(()).await?;
    // if the handshake is wrong, the server will terminate the connection

    let stream = handshake_enc.into_inner();
    let mut file = File::create(path).await?;

    stream.readable().await?;
    let mut dec = Framed::new(stream, FileChunkDecoder{});
    let mut current_offset = 0;

    loop {
        match dec.next().await {
            Some(Ok(chunk)) => {
                if current_offset != chunk.offset {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "offset mismatch").into());
                }
                current_offset += chunk.data.len() as u64;
                file.write(&chunk.data[..]).await?;
            },
            Some(Err(e)) => {
                return Err(e.into());
            },
            None => unreachable!()
        }
    }
}

/*
A linked list of all incremental chunks is maintained. 
The nodes can be read concurrently by many connection tasks.
Only the `next` field of the tail node will be contended by the writer once it is appended.
TODO: Is there an idiom or a crate that delivers similar functionality? This could potentially be replaced by tokio::sync::broadcast.
*/
struct Node {
    next : Arc<RwLock<Option<Node>>>,
    data : Vec<u8>
}

impl Node {
    fn new(data : Vec<u8>) -> Node { 
        Node { 
            next : Arc::new(RwLock::new(None)), data
        } 
    }
    async fn append(&mut self, node : Node) -> Arc<RwLock<Option<Node>>> {
        let mut n = self.next.write().await;
        *n = Some(node);
        return self.next.clone();
    }
}

/*
The server is more involving than the client. 
We have tasks to: 
1) Drain the file watch notify events. I believe Notify internally uses a thread, as it communicates 
using std::sync::mpsc queue, so we have a sync <-> async code communication. To avoid deadlock, the 
task yields to the scheduler after every message.
2) Extend the linked list of file appends with a new node. This waits on notification from (1) and 
performs async file read. 
3) Per connection tasks that wait on changes from (2) and send the new chunk to the recipents.
*/
async fn run_server(address : SocketAddr, path : &Path) -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind(&address)
        .await
        .expect("unable to bind TCP listener");

    let file_changed_notify_tx = Arc::new(Notify::new());
    let file_changed_notify_rx = file_changed_notify_tx.clone();
    
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

    // This is the task (1)
    tokio::spawn(async move {
        loop {
            let mut write_detected = false;
            // We call try_recv to avoid indefinitely stalling the task in case the file is not changed.
            // It might be better to use `recv_timeout` to avoid failing because of contention, but 
            // according to the doc it's broken.
            let mut drained = false;
            while !drained && !write_detected {
                match file_rx.try_recv() {
                    Ok(event) => {
                        match event {
                            notify::DebouncedEvent::Write(_) => {
                                write_detected = true;
                            }
                            _ => {}
                        }
                    },
                    Err(TryRecvError::Empty) => { drained = true; },
                    Err(TryRecvError::Disconnected) => {
                        return Err(TryRecvError::Disconnected);
                    }
                }
            }

            if write_detected {
                file_changed_notify_tx.notify_one();
            }
            // Return control to the scheduler
            tokio::task::yield_now().await;
        }

        #[allow(unreachable_code)]
        Ok::<_, TryRecvError>(())
    });

    let file_chunks_list_head = Arc::new(RwLock::new(Some(Node::new(buffer))));
    let mut write_head = file_chunks_list_head.clone();
    let (list_tx, list_rx) = watch::channel(());
    
    // This is the task (2)
    tokio::spawn(
        async move {
        let mut file = Box::pin(file);
        loop {
            file_changed_notify_rx.notified().await;
            let mut buffer = Vec::new();
            file.as_mut().read_to_end(&mut buffer).await?;

            if buffer.len() > 0 {
                let next_head;
                {
                    let mut write_guard = write_head.write().await;
                    let node = write_guard.as_mut().unwrap();
                    next_head = node.append(Node::new(buffer)).await;
                }
                write_head = next_head;
                list_tx.send(())?;
            }

        }
        // This is here because the compiler can't infer the return type
        #[allow(unreachable_code)]
        Ok::<_, Box<dyn Error + Send + Sync>>(())
    });

    loop {
        let (socket, _) = listener.accept().await?;
        let mut connection_list_rx = list_rx.clone();
        let connection_list_head = file_chunks_list_head.clone();
        
        // This is the task (3)
        tokio::spawn(async move {
            socket.readable().await?;

            let mut handshake_dec = Framed::new(socket, HandshakeDecoder{});
            if let Some(Err(e)) = handshake_dec.next().await {
                return Err(e.into());
            }

            let socket = handshake_dec.into_inner();

            let mut read_head = connection_list_head;
            let mut framed = Framed::new(socket, FileChunkEncoder{});
            let mut offset = 0;

            loop {
                let mut next_head = None;
                {
                    let read_guard = read_head.read().await;
                    if let Some(node) = read_guard.as_ref() {
                        next_head = Some(node.next.clone());
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
            // This is here because the compiler can't infer the return type
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
