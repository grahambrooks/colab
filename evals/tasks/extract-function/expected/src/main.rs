fn main() {
    let prices = vec![3, 5, 8];
    let total = total_price(prices);
    println!("{total}");
}

fn total_price(prices: Vec<i32>) -> i32 {
    let mut total = 0;
    for p in &prices {
        total += p;
    }
    total
}
