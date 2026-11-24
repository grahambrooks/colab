#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relevance() {
        assert!(LANG.is_source(Path::new("a.rs")));
        assert_eq!(LANG.is_source(Path::new("a.go")), false, "{}", describe("a.go"));
        let paths = vec![
            LANG.is_source(Path::new("b.rs")),
            vec![LANG.is_source(Path::new("c.rs"))].is_empty(),
        ];
        let label = format!("relevant: {}", LANG.is_source(Path::new("d.rs")));
        let kind = matches!(paths.first(), Some(_));
        let many = vec![is_relevant(Path::new("e.rs")); 3];
        let direct = LANG.is_source(Path::new("f.rs"));
    }
}
