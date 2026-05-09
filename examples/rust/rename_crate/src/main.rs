use old_tokio::sync::Mutex;

fn main() {
    let _ = Mutex::new(());
    println!("Hello from old_tokio");
}
