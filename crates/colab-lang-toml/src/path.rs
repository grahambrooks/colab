//! The path syntax `toml::key` targets.
//!
//! Segments are separated by `.`. A segment is a key, or a key followed by
//! a selector `[field=value]` naming the element of an array whose `field`
//! equals `value`:
//!
//! ```text
//! project.profile_version
//! repos[repo=local].hooks[id=gitleaks]
//! ```
//!
//! The array may be an array of tables (`[[repos]]`) or an array of inline
//! tables (`hooks = [{ id = "…" }]`). Keys cannot contain `.`, `[` or `]`.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Key(String),
    /// The element of array `key` whose `field` equals `value`.
    Select {
        key: String,
        field: String,
        value: String,
    },
}

impl Segment {
    pub fn key(&self) -> &str {
        match self {
            Segment::Key(key) | Segment::Select { key, .. } => key,
        }
    }
}

impl fmt::Display for Segment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Segment::Key(key) => write!(f, "{key}"),
            Segment::Select { key, field, value } => write!(f, "{key}[{field}={value}]"),
        }
    }
}

/// Parses a path, or says what is wrong with it.
pub fn parse(path: &str) -> Result<Vec<Segment>, String> {
    if path.trim().is_empty() {
        return Err("the path is empty".to_string());
    }
    let mut segments = Vec::new();
    let mut rest = path;
    loop {
        let end = rest.find(['.', '[']).unwrap_or(rest.len());
        let key = &rest[..end];
        if key.is_empty() || key.contains(']') {
            return Err(format!("`{path}` has an empty or malformed key"));
        }
        rest = &rest[end..];
        if let Some(selector) = rest.strip_prefix('[') {
            let close = selector
                .find(']')
                .ok_or_else(|| format!("`{path}` has a `[` without a closing `]`"))?;
            let (field, value) = selector[..close]
                .split_once('=')
                .ok_or_else(|| format!("`{path}`: a selector is written [field=value]"))?;
            let (field, value) = (field.trim(), value.trim());
            if field.is_empty() || value.is_empty() {
                return Err(format!(
                    "`{path}`: a selector needs both a field and a value"
                ));
            }
            segments.push(Segment::Select {
                key: key.to_string(),
                field: field.to_string(),
                value: value.to_string(),
            });
            rest = &selector[close + 1..];
        } else {
            segments.push(Segment::Key(key.to_string()));
        }
        if rest.is_empty() {
            return Ok(segments);
        }
        rest = rest
            .strip_prefix('.')
            .ok_or_else(|| format!("`{path}`: expected `.` after a selector"))?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_selectors_parse() {
        assert_eq!(
            parse("repos[repo=local].hooks[id=gitleaks]").unwrap(),
            vec![
                Segment::Select {
                    key: "repos".into(),
                    field: "repo".into(),
                    value: "local".into()
                },
                Segment::Select {
                    key: "hooks".into(),
                    field: "id".into(),
                    value: "gitleaks".into()
                },
            ]
        );
        assert_eq!(
            parse("project.profile_version").unwrap(),
            vec![
                Segment::Key("project".into()),
                Segment::Key("profile_version".into())
            ]
        );
    }

    #[test]
    fn malformed_paths_are_rejected() {
        for bad in [
            "", "a..b", ".a", "a.", "a[id=x", "a[idx]", "a[=x]", "a[id=x]b", "a]",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} parsed");
        }
    }
}
