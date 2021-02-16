extern crate tokio;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::sync::watch;
use tokio::time::{sleep, Duration};
use tokio::runtime::Builder;

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

fn main() {
    let head = Arc::new(RwLock::new(Some(Node::new(vec![0], 0))));
    let mut head2 = head.clone();
    let (tx, mut rx) = watch::channel(0);

    let rt = Builder::new_multi_thread().worker_threads(4).enable_time().enable_io().build().unwrap();

    rt.spawn(async move {
        let mut write_head = head;
        for i in 1..10 {
            let next_head;
            {
                let mut write_guard = write_head.write().await;
                let node = write_guard.as_mut().unwrap();
                next_head = node.append(Node::new(vec![i], node.gen + 1)).await;
            }
            write_head = next_head;
            tx.send(i);
            sleep(Duration::from_millis(100)).await;
            println!("{}", i);
        }
    });

    for w in 0..2 {
        let mut rx = rx.clone();
        let head2 = head2.clone();
        rt.spawn(
            async move {
                let w = w.clone() + 1;
                let mut read_head = head2;
                loop {
                    let mut next_head = None;
                    {
                        let read_guard = read_head.read().await;
                        if let Some(node) = read_guard.as_ref() {
                            next_head = Some(node.next.clone());
                            println!("w {}, gen {}, {}", w, node.gen, node.data[0]);
                        }
                    }
                    if let Some(next_head) = next_head {
                        read_head = next_head;
                    }
                    else {
                        rx.changed().await;
                    }
                }
            }
        );
    }

    rt.block_on(async { sleep(Duration::from_secs(5)).await; });
}