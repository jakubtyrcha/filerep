extern crate notify;
extern crate tokio;

use notify::{Watcher, RecursiveMode, watcher};
use tokio::sync::Notify;
use std::sync::Arc;
use tokio::time::{sleep,Duration};
use tokio::runtime::Builder;
use tokio::fs::File;
use std::io::{SeekFrom};
use tokio::io::AsyncSeekExt;


fn main() {
    let filepath = "test";

    let notify_file_changed = Arc::new(Notify::new());
    let notify_file_changed_receiver = notify_file_changed.clone();

    let (tx, mut rx) = std::sync::mpsc::channel();
    let mut watcher = watcher(tx, Duration::from_millis(1)).unwrap();
    watcher.watch(filepath, RecursiveMode::NonRecursive).unwrap();

    let rt = Builder::new_multi_thread().worker_threads(4).enable_time().enable_io().build().unwrap();

    rt.spawn(async move {
        let file = File::open(filepath).await.unwrap();
        let mut file = Box::pin(file);
        loop {
            println!("await permit");
            notify_file_changed_receiver.notified().await;
            println!("got permit");
            let seek_res = file.as_mut().seek(SeekFrom::End(0)).await.unwrap();
            println!("{}", seek_res);
        }
    });

    rt.block_on(async move {
        loop {
            match rx.recv() {
                Ok(event) => {
                    match event {
                        notify::DebouncedEvent::Write(_) => {
                            notify_file_changed.notify_one();
                            println!("issue permit");
                        }
                        _ => {}
                    }
                },
                _ => {}
            }
        }
    });
}