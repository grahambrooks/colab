use std::fs::File;
use std::fs::read;

static START: () = log("start");

fn load() {
    use std::fs::write;
    log("load");
    write("x", read("y").unwrap()).unwrap();
}
