//! JSON as `json::key` reads it: values with their byte ranges, objects'
//! members with their keys decoded, arrays' elements. Nothing else is
//! kept, since the edits are splices of the text.
//!
//! The JSON of RFC 8259, plus `//` and `/* */` comments between tokens (as
//! in `tsconfig.json` and editor settings), which are passed over. A
//! document that is not one such value, a trailing comma included, has no
//! tree, and is left alone.

use std::ops::Range;

/// Nesting deeper than this is not read (the reader recurses).
const MAX_DEPTH: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Object,
    Array,
    String,
    Number,
    True,
    False,
    Null,
}

impl Kind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Kind::Object => "object",
            Kind::Array => "array",
            Kind::String => "string",
            Kind::Number => "number",
            Kind::True => "true",
            Kind::False => "false",
            Kind::Null => "null",
        }
    }
}

#[derive(Debug)]
pub(crate) struct Value {
    pub kind: Kind,
    pub range: Range<usize>,
    /// An object's members, in order.
    pub members: Vec<Member>,
    /// An array's elements, in order.
    pub elements: Vec<Value>,
}

#[derive(Debug)]
pub(crate) struct Member {
    /// The key as decoded (`"a\/b"` is `a/b`).
    pub key: String,
    /// From the key's opening quote to the value's end.
    pub range: Range<usize>,
    pub value: Value,
}

/// The document's value, or `None` when `source` is not a JSON document.
pub(crate) fn parse(source: &str) -> Option<Value> {
    let mut r = Reader {
        b: source.as_bytes(),
        src: source,
        at: 0,
    };
    if source.starts_with('\u{feff}') {
        r.at = 3;
    }
    r.skip()?;
    let value = r.value(0)?;
    r.skip()?;
    (r.at == r.b.len()).then_some(value)
}

struct Reader<'s> {
    b: &'s [u8],
    src: &'s str,
    at: usize,
}

impl Reader<'_> {
    /// Past whitespace and comments; `None` at an unclosed `/*` or a lone `/`.
    fn skip(&mut self) -> Option<()> {
        loop {
            match self.b.get(self.at) {
                Some(b' ' | b'\t' | b'\n' | b'\r') => self.at += 1,
                Some(b'/') => match self.b.get(self.at + 1) {
                    Some(b'/') => {
                        self.at = self.src[self.at..]
                            .find('\n')
                            .map_or(self.b.len(), |i| self.at + i);
                    }
                    Some(b'*') => {
                        self.at = self.src[self.at + 2..].find("*/")? + self.at + 4;
                    }
                    _ => return None,
                },
                _ => return Some(()),
            }
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        let found = self.b.get(self.at) == Some(&c);
        if found {
            self.at += 1;
        }
        found
    }

    fn value(&mut self, depth: usize) -> Option<Value> {
        if depth > MAX_DEPTH {
            return None;
        }
        let start = self.at;
        let scalar = |kind, end| Value {
            kind,
            range: start..end,
            members: Vec::new(),
            elements: Vec::new(),
        };
        match *self.b.get(self.at)? {
            b'{' => self.object(depth),
            b'[' => self.array(depth),
            b'"' => {
                self.string()?;
                Some(scalar(Kind::String, self.at))
            }
            b'-' | b'0'..=b'9' => {
                self.number()?;
                Some(scalar(Kind::Number, self.at))
            }
            _ => {
                for (word, kind) in [
                    ("true", Kind::True),
                    ("false", Kind::False),
                    ("null", Kind::Null),
                ] {
                    if self.src[self.at..].starts_with(word) {
                        self.at += word.len();
                        return Some(scalar(kind, self.at));
                    }
                }
                None
            }
        }
    }

    fn object(&mut self, depth: usize) -> Option<Value> {
        let start = self.at;
        self.at += 1;
        let mut members = Vec::new();
        self.skip()?;
        if !self.eat(b'}') {
            loop {
                let key_start = self.at;
                self.string()?;
                let key = serde_json::from_str::<String>(&self.src[key_start..self.at]).ok()?;
                self.skip()?;
                if !self.eat(b':') {
                    return None;
                }
                self.skip()?;
                let value = self.value(depth + 1)?;
                members.push(Member {
                    key,
                    range: key_start..value.range.end,
                    value,
                });
                self.skip()?;
                if self.eat(b'}') {
                    break;
                }
                if !self.eat(b',') {
                    return None;
                }
                self.skip()?;
            }
        }
        Some(Value {
            kind: Kind::Object,
            range: start..self.at,
            members,
            elements: Vec::new(),
        })
    }

    fn array(&mut self, depth: usize) -> Option<Value> {
        let start = self.at;
        self.at += 1;
        let mut elements = Vec::new();
        self.skip()?;
        if !self.eat(b']') {
            loop {
                elements.push(self.value(depth + 1)?);
                self.skip()?;
                if self.eat(b']') {
                    break;
                }
                if !self.eat(b',') {
                    return None;
                }
                self.skip()?;
            }
        }
        Some(Value {
            kind: Kind::Array,
            range: start..self.at,
            members: Vec::new(),
            elements,
        })
    }

    /// A string at `"`, its escapes checked: past its closing quote.
    fn string(&mut self) -> Option<()> {
        if !self.eat(b'"') {
            return None;
        }
        loop {
            match *self.b.get(self.at)? {
                b'"' => {
                    self.at += 1;
                    return Some(());
                }
                b'\\' => match *self.b.get(self.at + 1)? {
                    b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => self.at += 2,
                    b'u' => {
                        let hex = self.b.get(self.at + 2..self.at + 6)?;
                        if !hex.iter().all(u8::is_ascii_hexdigit) {
                            return None;
                        }
                        self.at += 6;
                    }
                    _ => return None,
                },
                0..0x20 => return None,
                _ => self.at += 1,
            }
        }
    }

    /// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`
    fn number(&mut self) -> Option<()> {
        let digits = |r: &mut Self| {
            let from = r.at;
            while r.b.get(r.at).is_some_and(u8::is_ascii_digit) {
                r.at += 1;
            }
            r.at > from
        };
        self.eat(b'-');
        if !self.eat(b'0') && !digits(self) {
            return None;
        }
        if self.eat(b'.') && !digits(self) {
            return None;
        }
        if matches!(self.b.get(self.at), Some(b'e' | b'E')) {
            self.at += 1;
            if !self.eat(b'+') {
                self.eat(b'-');
            }
            if !digits(self) {
                return None;
            }
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_serde_json_reads() {
        for doc in [
            "{}",
            " [1, -2.5e+3, 0, \"a\\u00e9\\n\", true, false, null] ",
            "{\"a\": {\"b\": [{}]}, \"c\\/d\": \"\"}\n",
            "\u{feff}{\"bom\": 1}",
        ] {
            assert!(parse(doc).is_some(), "{doc}");
        }
        for doc in [
            "",
            "{",
            "{\"a\": }",
            "{\"a\": 1,}",
            "[1,]",
            "01",
            "1.",
            "-",
            "\"\t\"",
            "{} x",
            "{\"a\" 1}",
            "tru",
            "\"\\x\"",
            "/ {}",
        ] {
            assert!(parse(doc).is_none(), "{doc:?}");
        }
    }

    #[test]
    fn comments_are_passed_over() {
        let doc = "// settings\n{\n  /* one */ \"a\": 1, // two\n  \"b\": [2]\n}\n";
        let v = parse(doc).unwrap();
        let keys: Vec<_> = v.members.iter().map(|m| m.key.as_str()).collect();
        assert_eq!(keys, ["a", "b"]);
        assert_eq!(&doc[v.members[1].range.clone()], "\"b\": [2]");
        assert!(parse("{} /* open").is_none());
    }

    /// `COLAB_JSON_FILES=<dir> cargo test -p colab-lang-json -- --ignored`:
    /// every `.json` file under `dir` is read exactly when serde_json reads
    /// it (once comments are blanked), with the same top-level keys.
    #[test]
    #[ignore = "needs COLAB_JSON_FILES"]
    fn agrees_with_serde_json_over_a_tree() {
        let Ok(root) = std::env::var("COLAB_JSON_FILES") else {
            return;
        };
        let mut stack = vec![std::path::PathBuf::from(root)];
        let (mut checked, mut wrong) = (0, Vec::new());
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() && !kind.is_symlink() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                checked += 1;
                let ours = parse(&text);
                // Comments blanked, so serde_json sees the same offsets.
                let mut plain = text.clone().into_bytes();
                if ours.is_some() {
                    blank_comments(&text, &mut plain);
                }
                let plain = String::from_utf8(plain).unwrap_or_default();
                let theirs: Option<serde_json::Value> =
                    serde_json::from_str(plain.trim_start_matches('\u{feff}')).ok();
                let same = match (&ours, &theirs) {
                    (Some(o), Some(serde_json::Value::Object(t))) => {
                        let ours: std::collections::BTreeSet<&str> =
                            o.members.iter().map(|m| m.key.as_str()).collect();
                        ours.into_iter().eq(t.keys().map(String::as_str))
                    }
                    (Some(_), Some(_)) | (None, None) => true,
                    _ => false,
                };
                if !same {
                    wrong.push(format!(
                        "{}: ours {}, serde_json {}",
                        path.display(),
                        ours.is_some(),
                        theirs.is_some()
                    ));
                }
            }
        }
        eprintln!("{checked} files, {} disagree", wrong.len());
        for w in wrong.iter().take(30) {
            eprintln!("{w}");
        }
        assert!(wrong.is_empty());
    }

    /// Spaces over the comments outside `text`'s strings.
    fn blank_comments(text: &str, out: &mut [u8]) {
        let mut r = Reader {
            b: text.as_bytes(),
            src: text,
            at: 0,
        };
        let b = text.as_bytes();
        let mut i = 0;
        let mut in_string = false;
        while i < b.len() {
            if in_string {
                match b[i] {
                    b'\\' => i += 1,
                    b'"' => in_string = false,
                    _ => {}
                }
                i += 1;
                continue;
            }
            match b[i] {
                b'"' => {
                    in_string = true;
                    i += 1;
                }
                b'/' => {
                    r.at = i;
                    let _ = r.skip();
                    for c in &mut out[i..r.at.max(i + 1).min(b.len())] {
                        if *c != b'\n' {
                            *c = b' ';
                        }
                    }
                    i = r.at.max(i + 1);
                }
                _ => i += 1,
            }
        }
    }

    #[test]
    fn keys_are_decoded_and_ranges_exact() {
        let doc = "{\"a\\/b\" : [ 1 , {\"c\":null} ]}";
        let v = parse(doc).unwrap();
        let m = &v.members[0];
        assert_eq!(m.key, "a/b");
        assert_eq!(&doc[m.value.range.clone()], "[ 1 , {\"c\":null} ]");
        assert_eq!(m.value.elements[1].members[0].value.kind, Kind::Null);
        assert!(parse(&"[".repeat(MAX_DEPTH + 2)).is_none());
    }
}
