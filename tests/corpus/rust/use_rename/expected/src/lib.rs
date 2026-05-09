use async_tokio::sync::Mutex;
use async_tokio::fs;
use async_tokio as t;
use my_tokio::sync::Mutex as Other;

pub async fn hello() {
    let _: Mutex<()> = Mutex::new(());
    let _ = fs::read_to_string("hi");
    let _ = t::spawn(async {});
    let _: Other = Other::new(());
}
