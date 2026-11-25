//! Go's tokens, by the language specification: identifiers, keywords,
//! operators, the literals, comments, and the semicolons the specification
//! inserts at the end of a line (`Term`, on the newline itself, which a
//! tree keeps as whitespace inside the node it ends).

/// What a token is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum L {
    Ident,
    /// A keyword, or `nil` / `true` / `false` / `iota`.
    Kw(&'static str),
    Op(&'static str),
    Int,
    Float,
    Imag,
    Rune,
    /// `"…"`; `closed` when its `"` is there.
    Str {
        closed: bool,
    },
    /// `` `…` ``; `closed` when its `` ` `` is there.
    Raw {
        closed: bool,
    },
    /// The newline that ends a statement.
    Term,
    /// A character no token starts with.
    Bad,
    Eof,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Lx {
    pub k: L,
    pub start: usize,
    pub end: usize,
}

const KEYWORDS: &[&str] = &[
    "break",
    "case",
    "chan",
    "const",
    "continue",
    "default",
    "defer",
    "else",
    "fallthrough",
    "for",
    "func",
    "go",
    "goto",
    "if",
    "import",
    "interface",
    "map",
    "package",
    "range",
    "return",
    "select",
    "struct",
    "switch",
    "type",
    "var",
    "nil",
    "true",
    "false",
    "iota",
];

/// Longest first, so a prefix never wins.
const OPS: &[&str] = &[
    "&^=", "<<=", ">>=", "...", "&&", "||", "<-", "++", "--", "==", "!=", "<=", ">=", ":=", "+=",
    "-=", "*=", "/=", "%=", "&=", "|=", "^=", "&^", "<<", ">>", "+", "-", "*", "/", "%", "&", "|",
    "^", "<", ">", "=", "!", "(", ")", "[", "]", "{", "}", ",", ";", ".", ":", "~",
];

fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

fn is_ident_continue(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// Whether a line ending after `k` ends a statement.
fn ends_statement(k: L) -> bool {
    match k {
        L::Ident | L::Int | L::Float | L::Imag | L::Rune | L::Str { .. } | L::Raw { .. } => true,
        L::Kw(w) => matches!(
            w,
            "break" | "continue" | "fallthrough" | "return" | "nil" | "true" | "false" | "iota"
        ),
        L::Op(o) => matches!(o, "++" | "--" | ")" | "]" | "}"),
        _ => false,
    }
}

/// `src`'s tokens (with `Term`s, ending in `Eof`) and its comments.
pub(crate) fn lex(src: &str) -> (Vec<Lx>, Vec<(usize, usize)>) {
    let b = src.as_bytes();
    let mut toks: Vec<Lx> = Vec::new();
    let mut comments = Vec::new();
    let mut i = 0;
    let mut last = L::Eof;
    // The newline that ends a statement is the last one before the next
    // token or comment (tree-sitter-go reads the ones before it as
    // whitespace), so it is placed when that comes.
    let mut pending: Option<usize> = None;
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            if ends_statement(last) {
                pending = Some(i);
            }
            i += 1;
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // A byte-order mark at the start is no token.
        if i == 0 && src.starts_with('\u{feff}') {
            i += '\u{feff}'.len_utf8();
            continue;
        }
        if let Some(at) = pending.take() {
            toks.push(Lx {
                k: L::Term,
                start: at,
                end: at + 1,
            });
            last = L::Term;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            let end = src[i..].find('\n').map_or(b.len(), |p| i + p);
            comments.push((i, end));
            i = end;
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'*') {
            let end = src[i + 2..].find("*/").map_or(b.len(), |p| i + 2 + p + 2);
            comments.push((i, end));
            i = end;
            continue;
        }
        let start = i;
        let k = if is_ident_start(src[i..].chars().next().unwrap_or(' ')) {
            let mut j = i;
            for ch in src[i..].chars() {
                if !is_ident_continue(ch) {
                    break;
                }
                j += ch.len_utf8();
            }
            i = j;
            let word = &src[start..i];
            KEYWORDS
                .iter()
                .find(|k| **k == word)
                .map_or(L::Ident, |k| L::Kw(k))
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            let (k, end) = number(b, i);
            i = end;
            k
        } else if c == b'\'' {
            i = rune(src, i);
            L::Rune
        } else if c == b'"' {
            let mut j = i + 1;
            let mut closed = false;
            while j < b.len() {
                match b[j] {
                    b'\\' => j += 2,
                    b'"' => {
                        j += 1;
                        closed = true;
                        break;
                    }
                    b'\n' => break,
                    _ => j += 1,
                }
            }
            i = j.min(b.len());
            L::Str { closed }
        } else if c == b'`' {
            match src[i + 1..].find('`') {
                Some(p) => {
                    i = i + 1 + p + 1;
                    L::Raw { closed: true }
                }
                None => {
                    i = b.len();
                    L::Raw { closed: false }
                }
            }
        } else if let Some(op) = OPS.iter().find(|o| src[i..].starts_with(**o)) {
            i += op.len();
            L::Op(op)
        } else {
            i += src[i..].chars().next().map_or(1, char::len_utf8);
            L::Bad
        };
        toks.push(Lx { k, start, end: i });
        last = k;
    }
    if let Some(at) = pending {
        toks.push(Lx {
            k: L::Term,
            start: at,
            end: at + 1,
        });
    }
    toks.push(Lx {
        k: L::Eof,
        start: b.len(),
        end: b.len(),
    });
    (toks, comments)
}

fn digits(b: &[u8], mut i: usize, ok: impl Fn(u8) -> bool) -> usize {
    while i < b.len() && (ok(b[i]) || b[i] == b'_') {
        i += 1;
    }
    i
}

/// A numeric literal from `i`: its kind and end.
fn number(b: &[u8], mut i: usize) -> (L, usize) {
    let mut float = false;
    let hex = b[i] == b'0' && matches!(b.get(i + 1), Some(b'x' | b'X'));
    if hex {
        i = digits(b, i + 2, |c| c.is_ascii_hexdigit());
        if b.get(i) == Some(&b'.') {
            float = true;
            i = digits(b, i + 1, |c| c.is_ascii_hexdigit());
        }
        if matches!(b.get(i), Some(b'p' | b'P')) {
            float = true;
            i += 1;
            if matches!(b.get(i), Some(b'+' | b'-')) {
                i += 1;
            }
            i = digits(b, i, |c| c.is_ascii_digit());
        }
    } else if b[i] == b'0' && matches!(b.get(i + 1), Some(b'b' | b'B' | b'o' | b'O')) {
        i = digits(b, i + 2, |c| c.is_ascii_digit());
    } else {
        i = digits(b, i, |c| c.is_ascii_digit());
        if b.get(i) == Some(&b'.') {
            float = true;
            i = digits(b, i + 1, |c| c.is_ascii_digit());
        }
        if matches!(b.get(i), Some(b'e' | b'E')) {
            let mut j = i + 1;
            if matches!(b.get(j), Some(b'+' | b'-')) {
                j += 1;
            }
            if b.get(j).is_some_and(u8::is_ascii_digit) {
                float = true;
                i = digits(b, j, |c| c.is_ascii_digit());
            }
        }
    }
    if b.get(i) == Some(&b'i') {
        return (L::Imag, i + 1);
    }
    (if float { L::Float } else { L::Int }, i)
}

/// The end of a rune literal starting at `i`.
fn rune(src: &str, i: usize) -> usize {
    let b = src.as_bytes();
    let mut j = i + 1;
    if b.get(j) == Some(&b'\\') {
        j += 2;
    } else if let Some(c) = src[j..].chars().next() {
        j += c.len_utf8();
    }
    // Up to the closing quote on this line (octal, hex, `u` and `U` forms
    // run on).
    while j < b.len() && b[j] != b'\'' && b[j] != b'\n' {
        j += 1;
    }
    if b.get(j) == Some(&b'\'') { j + 1 } else { j }
}
