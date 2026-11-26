//! Links in a span of inline text (a paragraph's, a heading's, a cell's):
//! inline links `[text](destination "title")`, images
//! `![description](destination)`, and autolinks; code spans and backslash
//! escapes are read so that brackets inside them are not taken for links.
//! Emphasis and the rest of inline formatting are left as text: colab
//! reads links, not rendering.

use crate::tree::{Item, leaf, node, punct};

/// The link-like items in `text`, offsets within it.
pub(crate) fn items(text: &str) -> Vec<Item> {
    let mut out = Vec::new();
    scan(text, 0, text.len(), &mut out);
    out
}

fn scan(text: &str, from: usize, to: usize, out: &mut Vec<Item>) {
    let b = text.as_bytes();
    let mut i = from;
    while i < to {
        match b[i] {
            b'\\' => i += 2,
            b'`' => i = code_span(text, i, to, out),
            b'<' => i = autolink(text, i, to, out).unwrap_or(i + 1),
            b'!' if b.get(i + 1) == Some(&b'[') => match link(text, i + 1, to, true) {
                Some((item, end)) => {
                    out.push(item);
                    i = end;
                }
                None => i += 2,
            },
            b'[' => match link(text, i, to, false) {
                Some((item, end)) => {
                    out.push(item);
                    i = end;
                }
                None => i += 1,
            },
            _ => i += 1,
        }
    }
}

/// A code span at `i` (a run of backticks): the offset after it.
fn code_span(text: &str, i: usize, to: usize, out: &mut Vec<Item>) -> usize {
    let b = text.as_bytes();
    let n = b[i..to].iter().take_while(|c| **c == b'`').count();
    let mut j = i + n;
    while j < to {
        if b[j] == b'`' {
            let m = b[j..to].iter().take_while(|c| **c == b'`').count();
            if m == n {
                out.push(leaf("code_span", i..j + m));
                return j + m;
            }
            j += m;
        } else {
            j += 1;
        }
    }
    // No closing run: the backticks are text.
    i + n
}

/// `<scheme:…>` or `<user@host>` at `i`: the offset after it.
fn autolink(text: &str, i: usize, to: usize, out: &mut Vec<Item>) -> Option<usize> {
    let close = text[i + 1..to].find('>')? + i + 1;
    let inner = &text[i + 1..close];
    if inner.is_empty() || inner.contains([' ', '<', '\n']) {
        return None;
    }
    let scheme = inner.split(':').next().unwrap_or("");
    let uri = inner.contains(':')
        && (2..=32).contains(&scheme.len())
        && scheme
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c));
    let email = !uri && inner.contains('@') && !inner.contains(':');
    if !(uri || email) {
        return None;
    }
    let kind = if uri {
        "uri_autolink"
    } else {
        "email_autolink"
    };
    out.push(leaf(kind, i..close + 1));
    Some(close + 1)
}

/// The `]` matching the `[` at `open`, brackets nested, escapes and code
/// spans passed over.
fn close_bracket(text: &str, open: usize, to: usize) -> Option<usize> {
    let b = text.as_bytes();
    let mut depth = 0usize;
    let mut i = open;
    while i < to {
        match b[i] {
            b'\\' => i += 1,
            b'`' => {
                let mut ignore = Vec::new();
                let end = code_span(text, i, to, &mut ignore);
                if !ignore.is_empty() {
                    i = end;
                    continue;
                }
            }
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// An inline link (or, `image`, an image) whose `[` is at `open`: the item
/// and the offset after it.
fn link(text: &str, open: usize, to: usize, image: bool) -> Option<(Item, usize)> {
    let b = text.as_bytes();
    let close = close_bracket(text, open, to)?;
    if b.get(close + 1) != Some(&b'(') {
        return None;
    }
    let mut i = close + 2;
    let skip_ws = |i: &mut usize| {
        while *i < to && b[*i].is_ascii_whitespace() {
            *i += 1;
        }
    };
    skip_ws(&mut i);
    let mut dest = None;
    if i < to && b[i] == b'<' {
        let end = text[i + 1..to].find(['>', '\n'])? + i + 1;
        if b[end] != b'>' {
            return None;
        }
        // Kept with its brackets, as written.
        dest = Some(i..end + 1);
        i = end + 1;
    } else {
        let s = i;
        let mut depth = 0i32;
        while i < to && !b[i].is_ascii_whitespace() && !b[i].is_ascii_control() {
            match b[i] {
                b'\\' => i += 1,
                b'(' => depth += 1,
                b')' => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
            i += 1;
        }
        let i_end = i.min(to);
        i = i_end;
        if i > s {
            dest = Some(s..i);
        }
    }
    let ws = i;
    skip_ws(&mut i);
    let mut title = None;
    if i < to && i > ws && matches!(b[i], b'"' | b'\'' | b'(') {
        let closing = match b[i] {
            b'(' => ')',
            c => c as char,
        };
        let end = text[i + 1..to].find(closing)? + i + 1;
        title = Some(i..end + 1);
        i = end + 1;
        skip_ws(&mut i);
    }
    if i >= to || b[i] != b')' {
        return None;
    }
    let text_kind = if image {
        "image_description"
    } else {
        "link_text"
    };
    let start = if image { open - 1 } else { open };
    let mut items = Vec::new();
    if image {
        items.push(punct("!", open - 1..open));
    }
    items.push(punct("[", open..open + 1));
    if close > open + 1 {
        // A link's text may hold images (a badge that links somewhere).
        let mut inner = Vec::new();
        scan(text, open + 1, close, &mut inner);
        if inner.is_empty() {
            items.push(leaf(text_kind, open + 1..close));
        } else {
            let mut parts = Vec::new();
            let mut at = open + 1;
            for item in inner {
                if item.start() > at {
                    parts.push(Item::Gap(at..item.start()));
                }
                at = item.end();
                parts.push(item);
            }
            if close > at {
                parts.push(Item::Gap(at..close));
            }
            items.push(node(text_kind, parts));
        }
    }
    items.push(punct("]", close..close + 1));
    items.push(punct("(", close + 1..close + 2));
    if let Some(d) = dest {
        items.push(leaf("link_destination", d));
    }
    if let Some(t) = title {
        items.push(leaf("link_title", t));
    }
    items.push(punct(")", i..i + 1));
    let _ = start;
    Some((
        node(if image { "image" } else { "inline_link" }, items),
        i + 1,
    ))
}
