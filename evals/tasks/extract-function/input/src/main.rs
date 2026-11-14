fn main() {
    let prices = vec![3, 5, 8];
    let mut total = 0;
    for p in &prices {
        total += p;
    }
    println!("{total}");
}
