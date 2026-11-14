//! `let $NEW = $FROM | step | …`: a value derived from a binding, for a
//! `<lang>::pattern` `replace` template.
//!
//! ```text
//! match go::pattern 'func ($R $T) $F() $OUT { $$$B }' where $F matches /^Get[A-Z]/ {
//!     let $NAME = $F | strip_prefix("Get")
//!     replace 'func ($R $T) $NAME() $OUT { $$$B }'
//! }
//! ```
//!
//! A metavariable stands for a whole node, so `Get$NAME` cannot match
//! part of an identifier; a `let` computes that part from the whole. The
//! steps are plain data in `colab_core` ([`Transform`]); this module
//! compiles their regexes once and applies them to a binding's text.

use colab_core::Transform;

/// One compiled step.
#[derive(Debug)]
pub(crate) enum Step {
    Plain(Transform),
    Replace { regex: regex::Regex, with: String },
}

impl Step {
    /// `step` compiled: its regex, if it has one, checked here, so a bad
    /// one is a compile-time error naming the step.
    pub(crate) fn compile(step: &Transform) -> Result<Self, String> {
        match step {
            Transform::Replace { regex, with } => {
                // `/` ends the DSL's regex literal, so it is written `\/`.
                let compiled = regex::Regex::new(&regex.replace(r"\/", "/"))
                    .map_err(|e| format!("`{step}`: {e}"))?;
                Ok(Self::Replace {
                    regex: compiled,
                    with: with.clone(),
                })
            }
            other => Ok(Self::Plain(other.clone())),
        }
    }

    pub(crate) fn apply(&self, text: &str) -> String {
        match self {
            Self::Replace { regex, with } => regex.replace_all(text, with.as_str()).into_owned(),
            Self::Plain(t) => apply(t, text),
        }
    }
}

fn apply(step: &Transform, text: &str) -> String {
    match step {
        Transform::Lower => text.to_lowercase(),
        Transform::Upper => text.to_uppercase(),
        Transform::Capitalize => capitalize(text),
        Transform::Snake => cased(text, "_", |w, _| w.to_lowercase()),
        Transform::Kebab => cased(text, "-", |w, _| w.to_lowercase()),
        Transform::UpperSnake => cased(text, "_", |w, _| w.to_uppercase()),
        Transform::Camel => cased(text, "", |w, i| {
            if i == 0 {
                w.to_lowercase()
            } else {
                capitalize(&w.to_lowercase())
            }
        }),
        Transform::Pascal => cased(text, "", |w, _| capitalize(&w.to_lowercase())),
        Transform::StripPrefix(p) => text.strip_prefix(p.as_str()).unwrap_or(text).to_string(),
        Transform::StripSuffix(s) => text.strip_suffix(s.as_str()).unwrap_or(text).to_string(),
        Transform::Substring { start, end } => {
            let chars: Vec<char> = text.chars().collect();
            let len = chars.len() as i64;
            let at = |i: i64| (if i < 0 { len + i } else { i }).clamp(0, len) as usize;
            let (from, to) = (at(*start), end.map_or(chars.len(), at));
            if from >= to {
                String::new()
            } else {
                chars[from..to].iter().collect()
            }
        }
        // Applied by the pattern engine, which has the code it needs.
        Transform::Rewrite(_) => text.to_string(),
        // Compiled into `Step::Replace`; kept total for a plain call.
        Transform::Replace { regex, with } => regex::Regex::new(regex)
            .map(|re| re.replace_all(text, with.as_str()).into_owned())
            .unwrap_or_else(|_| text.to_string()),
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `text` split into words, each passed through `word` (with its index),
/// joined with `sep`. Leading and trailing underscores are kept, so
/// `_privateName` stays private as `_private_name`.
fn cased(text: &str, sep: &str, word: impl Fn(&str, usize) -> String) -> String {
    let core = text.trim_matches('_');
    let lead = &text[..text.len() - text.trim_start_matches('_').len()];
    let trail = &text[text.trim_end_matches('_').len()..];
    if core.is_empty() {
        return text.to_string();
    }
    let words: Vec<String> = words(core)
        .iter()
        .enumerate()
        .map(|(i, w)| word(w, i))
        .collect();
    format!("{lead}{}{trail}", words.join(sep))
}

/// The words of an identifier: split at `_`, `-`, whitespace and `.`,
/// before an upper-case letter that follows a lower-case one or a digit
/// (`userId` → `user`, `Id`), and before the last capital of an acronym
/// that a lower-case letter follows (`HTTPServer` → `HTTP`, `Server`).
pub(crate) fn words(text: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (k, &(i, c)) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if let Some(s) = start.take() {
                out.push(&text[s..i]);
            }
            continue;
        }
        let prev = k.checked_sub(1).map(|p| chars[p].1);
        let next = chars.get(k + 1).map(|&(_, n)| n);
        let boundary = c.is_uppercase()
            && prev.is_some_and(|p| {
                p.is_lowercase()
                    || p.is_ascii_digit()
                    || (p.is_uppercase() && next.is_some_and(char::is_lowercase))
            });
        match start {
            Some(s) if boundary => {
                out.push(&text[s..i]);
                start = Some(i);
            }
            None => start = Some(i),
            Some(_) => {}
        }
    }
    if let Some(s) = start {
        out.push(&text[s..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(step: Transform, text: &str) -> String {
        Step::compile(&step).unwrap().apply(text)
    }

    #[test]
    fn identifiers_split_at_separators_and_case_changes() {
        assert_eq!(words("HTTPServerError"), ["HTTP", "Server", "Error"]);
        assert_eq!(words("user_id-v2 x"), ["user", "id", "v2", "x"]);
        assert_eq!(words("parseJSON2Map"), ["parse", "JSON2", "Map"]);
        assert_eq!(words("getUserID"), ["get", "User", "ID"]);
    }

    #[test]
    fn case_conversions() {
        assert_eq!(run(Transform::Snake, "getUserID"), "get_user_id");
        assert_eq!(run(Transform::Snake, "_privateName"), "_private_name");
        assert_eq!(run(Transform::Kebab, "HTTPServer"), "http-server");
        assert_eq!(run(Transform::Camel, "user_id"), "userId");
        assert_eq!(run(Transform::Pascal, "user_id"), "UserId");
        assert_eq!(run(Transform::UpperSnake, "maxRetries"), "MAX_RETRIES");
        assert_eq!(run(Transform::Capitalize, "name"), "Name");
        assert_eq!(run(Transform::Lower, "ABC"), "abc");
        assert_eq!(run(Transform::Snake, "__"), "__");
    }

    #[test]
    fn affixes_regexes_and_substrings() {
        assert_eq!(run(Transform::StripPrefix("Get".into()), "GetName"), "Name");
        assert_eq!(run(Transform::StripPrefix("Get".into()), "Name"), "Name");
        assert_eq!(
            run(Transform::StripSuffix("Impl".into()), "RepoImpl"),
            "Repo"
        );
        let re = Transform::Replace {
            regex: "^(is|has)_".into(),
            with: "${1}".into(),
        };
        assert_eq!(run(re, "has_items"), "hasitems");
        let quoted = Transform::Substring {
            start: 1,
            end: Some(-1),
        };
        assert_eq!(run(quoted, "\"text\""), "text");
        let tail = Transform::Substring {
            start: -3,
            end: None,
        };
        assert_eq!(run(tail, "abcdef"), "def");
        let empty = Transform::Substring {
            start: 4,
            end: Some(2),
        };
        assert_eq!(run(empty, "abcdef"), "");
        let bad = Transform::Replace {
            regex: "(".into(),
            with: String::new(),
        };
        assert!(Step::compile(&bad).unwrap_err().contains("replace(/(/"));
    }
}
