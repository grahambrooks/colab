//! JSON Pointer (RFC 6901) parsing.

/// The reference tokens of a pointer: `/a/b~1c` gives `["a", "b/c"]`.
/// The whole-document pointer `""` is not accepted: `json::key` edits
/// members, not the document itself.
pub fn parse(pointer: &str) -> Result<Vec<String>, String> {
    let Some(rest) = pointer.strip_prefix('/') else {
        return Err(format!(
            "`{pointer}` is not a JSON Pointer: it must start with `/`, such as /enabledPlugins"
        ));
    };
    rest.split('/')
        .map(|token| {
            let mut out = String::with_capacity(token.len());
            let mut chars = token.chars();
            while let Some(c) = chars.next() {
                if c != '~' {
                    out.push(c);
                    continue;
                }
                match chars.next() {
                    Some('0') => out.push('~'),
                    Some('1') => out.push('/'),
                    _ => {
                        return Err(format!(
                            "`{pointer}`: `~` must be written `~0`, and `/` inside a key `~1`"
                        ));
                    }
                }
            }
            Ok(out)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_unescaped() {
        assert_eq!(
            parse("/enabledPlugins/a~1b~0c").unwrap(),
            vec!["enabledPlugins", "a/b~c"]
        );
        assert_eq!(parse("/").unwrap(), vec![""]);
    }

    #[test]
    fn malformed_pointers_are_rejected() {
        for bad in ["", "a/b", "/a~2", "/a~"] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }
}
