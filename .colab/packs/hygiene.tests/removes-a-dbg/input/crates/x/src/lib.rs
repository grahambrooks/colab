pub fn f(x: u32) -> u32 {
    dbg!(x);
    let y = x + 1;
    dbg!(x, y);
    y
}
