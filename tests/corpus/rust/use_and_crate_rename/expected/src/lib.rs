use tokio::sync::Mutex;

pub fn make() -> Mutex<()> {
    Mutex::new(())
}
