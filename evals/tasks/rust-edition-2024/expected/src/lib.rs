use std::env;

unsafe extern "C" {
    fn abs(x: i32) -> i32;
}

unsafe extern "C" {
    fn labs(x: i64) -> i64;
}

#[unsafe(no_mangle)]
pub extern "C" fn exported() -> i32 {
    unsafe { abs(-1) }
}

#[unsafe(export_name = "renamed")]
pub fn named() {}

pub fn configure(r#gen: u32) {
    unsafe { env::set_var("MODE", "fast") };
    unsafe { std::env::remove_var("OLD") };
    unsafe {
        std::env::set_var("SAFE", "already");
    }
    let _ = r#gen;
}

pub struct Seeds {
    pub r#gen: u64,
}

pub fn first(s: &Seeds) -> u64 {
    s.r#gen
}
