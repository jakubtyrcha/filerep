use tokio::sync::Notify;
use std::sync::Arc;

// TODO
// replace with const_format when stable
fn get_handshake_msg() -> Vec<u8> {
    Vec::from(format!("filerep v{}", 3.0).as_bytes())
}

#[tokio::main]
async fn main() {
    let notify = Arc::new(Notify::new());
    let notify2 = notify.clone();

    tokio::spawn(async move {
        notify2.notified().await;
        println!("received notification");
    });

    println!("sending notification");
    notify.notify_one();
}