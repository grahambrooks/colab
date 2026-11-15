mod mine {
    pub fn read(_p: &str) -> String {
        String::new()
    }
}

fn main() {
    let a: Option<u32> = Some(1);
    let b: Result<u32, String> = Ok(2);
    let n = a.unwrap();
    let m = b.unwrap();
    let x = std::fs::read("f").unwrap_or_default();
    let y = mine::read("f");
    println!("{n} {m} {x:?} {y}");
}
