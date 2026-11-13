use std::fmt::Write;
use std::collections::HashMap;

fn unused_helper() -> u32 {
    7
}

fn total(prices: &[u32]) -> u32 {
    let mut sum = 0;
    for p in prices {
        sum += p * 2 + 1;
    }
    sum
}

fn main() {
    let mut s = String::new();
    let m: HashMap<u8, u8> = HashMap::new();
    write!(s, "{} {}", total(&[1, 2]), m.len()).unwrap();
    println!("{s}");
}
