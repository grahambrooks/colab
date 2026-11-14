use std::env;

extern "C" {
    fn abs(x: i32) -> i32;
}

unsafe extern "C" {
    fn labs(x: i64) -> i64;
}

#[no_mangle]
pub extern "C" fn exported() -> i32 {
    unsafe { abs(-1) }
}

#[export_name = "renamed"]
pub fn named() {}

#[link_section = "__DATA,__colab"]
#[used]
pub static MARKER: u32 = 1;

pub fn configure(gen: u32) {
    env::set_var("MODE", "fast");
    std::env::set_var("LEVEL", "2");
    std::env::remove_var("OLD");
    env::remove_var("STALE");
    unsafe {
        std::env::set_var("SAFE", "already");
    }
    let _ = gen;
}

pub struct Seeds {
    pub gen: u64,
}

pub fn first(s: &Seeds) -> u64 {
    s.gen
}
