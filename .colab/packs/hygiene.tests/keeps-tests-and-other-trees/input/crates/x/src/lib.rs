pub fn f(x: u32) -> u32 {
    let y = dbg!(x) + 1;
    y
}

#[cfg(test)]
mod tests {
    #[test]
    fn t() {
        dbg!(super::f(1));
    }
}
