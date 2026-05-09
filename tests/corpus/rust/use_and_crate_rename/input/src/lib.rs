use old_tokio::sync::Mutex;

pub fn make() -> Mutex<()> {
    Mutex::new(())
}
