#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relevance() {
        assert!(is_relevant(Path::new("a.rs")));
        assert_eq!(is_relevant(Path::new("a.go")), false, "{}", describe("a.go"));
        let paths = vec![
            is_relevant(Path::new("b.rs")),
            legacy_flag(1),
            vec![is_relevant(Path::new("c.rs"))].is_empty(),
        ];
        let label = format!("relevant: {}", is_relevant(Path::new("d.rs")));
        let kind = matches!(paths.first(), Some(_));
        let many = vec![is_relevant(Path::new("e.rs")); 3];
        let direct = is_relevant(Path::new("f.rs"));
    }
}
