use std::io::File;
use std::io::read;

static START: () = trace("start");

fn load() {
    use std::fs::write;
    trace("load");
    write("x", read("y").unwrap()).unwrap();
}
