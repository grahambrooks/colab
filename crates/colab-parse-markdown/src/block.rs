//! Markdown's blocks, line by line (CommonMark, with GFM pipe tables and
//! task markers): front matter, headings, paragraphs, lists and their
//! items, block quotes, fenced and indented code, HTML blocks, thematic
//! breaks, pipe tables and link reference definitions. A container (a
//! block quote, a list item) strips its prefix from its lines and parses
//! them as blocks of their own.

use crate::tree::{Item, block, leaf, node, node_to, punct};

/// One line of a container: where it starts in the file, where the
/// container's content starts on it, its end (before `\n`), and the start
/// of the next line.
#[derive(Clone, Copy, Debug)]
struct Line {
    content: usize,
    end: usize,
    next: usize,
}

pub(crate) struct Blocks<'s> {
    src: &'s str,
}

/// The lines of `src` from `from`.
fn lines(src: &str, from: usize) -> Vec<Line> {
    let mut out = Vec::new();
    let mut at = from;
    while at < src.len() {
        let nl = src[at..].find('\n').map_or(src.len(), |p| at + p);
        let end = if nl > at && src.as_bytes()[nl - 1] == b'\r' {
            nl - 1
        } else {
            nl
        };
        let next = (nl + 1).min(src.len());
        out.push(Line {
            content: at,
            end,
            next: if nl == src.len() { src.len() } else { next },
        });
        at = if nl == src.len() { src.len() } else { next };
    }
    out
}

impl<'s> Blocks<'s> {
    pub fn new(src: &'s str) -> Self {
        Self { src }
    }

    fn text(&self, l: Line) -> &'s str {
        &self.src[l.content..l.end]
    }

    fn blank(&self, l: Line) -> bool {
        self.text(l).trim().is_empty()
    }

    /// Leading whitespace: (columns, bytes).
    fn indent(&self, l: Line) -> (usize, usize) {
        let mut cols = 0;
        let mut bytes = 0;
        for b in self.text(l).bytes() {
            match b {
                b' ' => cols += 1,
                b'\t' => cols += 4 - cols % 4,
                _ => break,
            }
            bytes += 1;
        }
        (cols, bytes)
    }

    /// The line's text after up to three spaces of indentation, and where
    /// it starts; `None` when indented four or more (code).
    fn lead(&self, l: Line) -> Option<(&'s str, usize)> {
        let (cols, bytes) = self.indent(l);
        (cols < 4).then(|| (&self.src[l.content + bytes..l.end], l.content + bytes))
    }

    pub fn document(&self) -> Vec<Item> {
        let mut items = Vec::new();
        let mut from = 0;
        for (fence, kind) in [("---", "minus_metadata"), ("+++", "plus_metadata")] {
            if let Some(end) = self.front_matter(fence) {
                items.push(block(kind, 0..end));
                from = end;
                break;
            }
        }
        items.extend(self.blocks(&lines(self.src, from)));
        items
    }

    /// The end of front matter fenced by `fence` lines at the very start.
    fn front_matter(&self, fence: &str) -> Option<usize> {
        let first = self.src.lines().next()?;
        if first.trim_end() != fence {
            return None;
        }
        let ls = lines(self.src, 0);
        let close = ls
            .iter()
            .skip(1)
            .find(|l| self.text(**l).trim_end() == fence)?;
        Some(close.end)
    }

    fn blocks(&self, lines: &[Line]) -> Vec<Item> {
        let mut out: Vec<Item> = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let l = lines[i];
            if self.blank(l) {
                i += 1;
                continue;
            }
            let Some((t, at)) = self.lead(l) else {
                i = self.indented_code(lines, i, &mut out);
                continue;
            };
            if let Some(n) = self.fence(lines, i, t, at, &mut out) {
                i = n;
            } else if let Some(level) = atx_level(t) {
                out.push(self.atx(l, t, at, level));
                i += 1;
            } else if thematic_break(t) {
                out.push(node_to(
                    "thematic_break",
                    vec![leaf("thematic_break", at..at + t.trim_end().len())],
                    l.next,
                ));
                i += 1;
            } else if let Some(end) = self.html(lines, i, t, false) {
                let last = lines[end - 1];
                out.push(block("html_block", at..last.next));
                i = end;
            } else if t.starts_with('>') {
                i = self.quote(lines, i, &mut out);
            } else if list_marker(t).is_some() {
                i = self.list(lines, i, &mut out);
            } else {
                i = self.paragraph(lines, i, &mut out);
            }
        }
        // Blank lines after a block belong to the list item or list it
        // ends; elsewhere they are the container's.
        out
    }

    fn indented_code(&self, lines: &[Line], start: usize, out: &mut Vec<Item>) -> usize {
        let mut last = start;
        let mut i = start + 1;
        while i < lines.len() {
            if self.blank(lines[i]) {
                i += 1;
                continue;
            }
            if self.indent(lines[i]).0 < 4 {
                break;
            }
            last = i;
            i += 1;
        }
        let l = lines[start];
        out.push(block("indented_code_block", l.content..lines[last].next));
        last + 1
    }

    fn atx(&self, l: Line, t: &str, at: usize, level: usize) -> Item {
        let marker = [
            "atx_h1_marker",
            "atx_h2_marker",
            "atx_h3_marker",
            "atx_h4_marker",
            "atx_h5_marker",
            "atx_h6_marker",
        ][level - 1];
        let mut items = vec![leaf(marker, at..at + level)];
        let rest = &t[level..];
        let lead = rest.len() - rest.trim_start().len();
        let content_start = at + level + lead;
        let content_end = at + level + rest.trim_end().len();
        if content_end > content_start {
            items.push(leaf("inline", content_start..content_end).with("heading_content"));
        }
        node_to("atx_heading", items, l.next)
    }

    /// A fenced code block opening at line `i`: the line after it.
    fn fence(
        &self,
        lines: &[Line],
        i: usize,
        t: &str,
        at: usize,
        out: &mut Vec<Item>,
    ) -> Option<usize> {
        let ch = t.chars().next().filter(|c| *c == '`' || *c == '~')?;
        let n = t.chars().take_while(|c| *c == ch).count();
        if n < 3 {
            return None;
        }
        let info = &t[n..];
        if ch == '`' && info.contains('`') {
            return None;
        }
        let l = lines[i];
        // A delimiter is its run of backticks or tildes alone, so the text
        // before it on its line is the fence's indentation (`Fence::indent`).
        let mut items = vec![leaf("fenced_code_block_delimiter", at..at + n)];
        let info_trimmed = info.trim();
        if !info_trimmed.is_empty() {
            let istart = at + n + (info.len() - info.trim_start().len());
            let iend = istart + info_trimmed.len();
            let word = info_trimmed.split_whitespace().next().unwrap_or("");
            let lang = leaf("language", istart..istart + word.len());
            let mut parts = vec![lang];
            if iend > istart + word.len() {
                parts.push(Item::Gap(istart + word.len()..iend));
            }
            items.push(node("info_string", parts).with("info_string"));
        }
        // Up to the closing fence: the same character, at least as many,
        // indented less than four, nothing after.
        let mut j = i + 1;
        let mut close = None;
        while j < lines.len() {
            if let Some((ct, cat)) = self.lead(lines[j]) {
                let m = ct.chars().take_while(|c| *c == ch).count();
                if m >= n && ct[m..].trim().is_empty() {
                    close = Some((j, cat, m));
                    break;
                }
            }
            j += 1;
        }
        let content_end = close.map_or(lines.last().map_or(l.next, |x| x.next), |(c, _, _)| {
            lines[c].content
        });
        if i + 1 < lines.len() && lines[i + 1].content < content_end {
            items.push(block(
                "code_fence_content",
                lines[i + 1].content..content_end,
            ));
        }
        let (end_line, next) = match close {
            Some((c, cat, m)) => {
                items.push(leaf("fenced_code_block_delimiter", cat..cat + m));
                (lines[c], c + 1)
            }
            None => (*lines.last().unwrap_or(&l), lines.len()),
        };
        out.push(node_to("fenced_code_block", items, end_line.next));
        Some(next)
    }

    /// An HTML block starting at line `i`: the index after its last line.
    /// `in_paragraph`: whether it would interrupt one (type 7 cannot).
    fn html(&self, lines: &[Line], i: usize, t: &str, in_paragraph: bool) -> Option<usize> {
        let kind = html_start(t, in_paragraph)?;
        let ends = |line: &str| -> bool {
            let lower = line.to_ascii_lowercase();
            match kind {
                1 => ["</script>", "</pre>", "</style>", "</textarea>"]
                    .iter()
                    .any(|e| lower.contains(e)),
                2 => line.contains("-->"),
                3 => line.contains("?>"),
                4 => line.contains('>'),
                5 => line.contains("]]>"),
                _ => false,
            }
        };
        let mut j = i;
        while j < lines.len() {
            if kind >= 6 {
                if j > i && self.blank(lines[j]) {
                    return Some(j);
                }
            } else if ends(self.text(lines[j])) {
                return Some(j + 1);
            }
            j += 1;
        }
        Some(lines.len())
    }

    /// Whether line `l` starts a block that ends a paragraph.
    fn interrupts(&self, l: Line) -> bool {
        let Some((t, _)) = self.lead(l) else {
            return false;
        };
        atx_level(t).is_some()
            || thematic_break(t)
            || t.starts_with('>')
            || t.starts_with("```")
            || t.starts_with("~~~")
            || html_start(t, true).is_some()
            || list_marker(t).is_some_and(|m| m.interrupts)
    }

    fn quote(&self, lines: &[Line], start: usize, out: &mut Vec<Item>) -> usize {
        let mut inner: Vec<Line> = Vec::new();
        let mut i = start;
        let mut lazy_ok = false;
        while i < lines.len() {
            let l = lines[i];
            match self.lead(l) {
                Some((t, at)) if t.starts_with('>') => {
                    let mut c = at + 1;
                    if self.src.as_bytes().get(c) == Some(&b' ') {
                        c += 1;
                    }
                    let c = c.min(l.end);
                    inner.push(Line { content: c, ..l });
                    lazy_ok = !self.src[c..l.end].trim().is_empty();
                }
                _ if lazy_ok && !self.blank(l) && !self.interrupts(l) => inner.push(l),
                _ => break,
            }
            i += 1;
        }
        let first = lines[start];
        let at = self.lead(first).map_or(first.content, |(_, a)| a);
        let mut items = vec![leaf("block_quote_marker", at..at + 1)];
        items.extend(self.blocks(&inner));
        out.push(node_to("block_quote", items, lines[i - 1].next));
        i
    }

    fn list(&self, lines: &[Line], start: usize, out: &mut Vec<Item>) -> usize {
        let mut items = Vec::new();
        let mut i = start;
        let first = list_marker(self.lead(lines[start]).map_or("", |x| x.0));
        let Some(first) = first else {
            return start + 1;
        };
        while i < lines.len() {
            let Some((t, at)) = self.lead(lines[i]) else {
                break;
            };
            let Some(m) = list_marker(t) else {
                break;
            };
            if !m.same_list(&first) || thematic_break(t) {
                break;
            }
            let (item, next) = self.item(lines, i, at, &m);
            items.push(item);
            i = next;
        }
        let end = items.last().map_or(0, Item::end);
        // The list starts at its first line, indentation included.
        let line_start = lines[start].content;
        if items.first().is_some_and(|f| f.start() > line_start) {
            let first = items.first().map_or(line_start, Item::start);
            items.insert(0, Item::Gap(line_start..first));
        }
        out.push(node_to("list", items, end));
        i
    }

    /// The list item whose marker is at `at` on line `start`, and the line
    /// after it (its trailing blank lines included).
    fn item(&self, lines: &[Line], start: usize, at: usize, m: &Marker) -> (Item, usize) {
        let l = lines[start];
        let marker_end = at + m.width;
        // Spaces after the marker: up to four count; more starts code.
        let after = &self.src[marker_end..l.end];
        let spaces = after.len() - after.trim_start_matches(' ').len();
        let (spaces, empty) = if after.trim().is_empty() {
            (usize::from(!after.is_empty()), true)
        } else if spaces > 4 {
            (1, false)
        } else {
            (spaces, false)
        };
        let content_col = (at - l.content) + m.width + spaces;
        let mut items = vec![leaf(m.kind, at..marker_end + spaces)];
        let mut inner = vec![Line {
            content: (marker_end + spaces).min(l.end),
            ..l
        }];
        // A task marker right after the list marker.
        let rest = &self.src[(marker_end + spaces).min(l.end)..l.end];
        if !empty
            && (rest.starts_with("[ ] ") || rest.starts_with("[x] ") || rest.starts_with("[X] "))
        {
            let s = marker_end + spaces;
            let kind = if rest.starts_with("[ ]") {
                "task_list_marker_unchecked"
            } else {
                "task_list_marker_checked"
            };
            items.push(leaf(kind, s..s + 3));
            inner[0].content = s + 4;
        }
        let mut i = start + 1;
        let mut last_text = !empty;
        let mut end = l.next;
        let mut pending_blank = Vec::new();
        while i < lines.len() {
            let line = lines[i];
            if self.blank(line) {
                pending_blank.push(line);
                i += 1;
                continue;
            }
            let (cols, bytes) = self.indent(line);
            if cols >= content_col {
                // Its content: the item's indentation removed.
                let mut strip = 0;
                let mut c = 0;
                for b in self.src[line.content..line.content + bytes].bytes() {
                    if c >= content_col {
                        break;
                    }
                    c += if b == b'\t' { 4 - c % 4 } else { 1 };
                    strip += 1;
                }
                inner.append(&mut pending_blank);
                inner.push(Line {
                    content: line.content + strip,
                    ..line
                });
                last_text = true;
                end = line.next;
                i += 1;
                continue;
            }
            // A lazy continuation of the item's paragraph; any list marker
            // starts the next item instead (`2.` too, which could not
            // interrupt a paragraph outside a list).
            let marker = self.lead(line).and_then(|(t, _)| list_marker(t)).is_some();
            if pending_blank.is_empty() && last_text && !marker && !self.interrupts(line) {
                inner.push(line);
                end = line.next;
                i += 1;
                continue;
            }
            break;
        }
        // The blank lines after it are its own, and its last block's (a
        // nested list item takes them too).
        if let Some(b) = pending_blank.last() {
            end = b.next;
            inner.append(&mut pending_blank);
        }
        let _ = m;
        items.extend(self.blocks(&inner));
        (node_to("list_item", items, end), i)
    }

    fn paragraph(&self, lines: &[Line], start: usize, out: &mut Vec<Item>) -> usize {
        // Link reference definitions first.
        let mut i = start;
        while i < lines.len() {
            let Some((t, at)) = self.lead(lines[i]) else {
                break;
            };
            let label_part = |label: std::ops::Range<usize>| {
                vec![
                    leaf("link_label", at + label.start..at + label.end),
                    punct(":", at + label.end..at + label.end + 1),
                ]
            };
            if let Some(def) = link_definition(t) {
                let mut parts = label_part(def.label);
                parts.push(leaf(
                    "link_destination",
                    at + def.dest.start..at + def.dest.end,
                ));
                if let Some(title) = def.title {
                    parts.push(leaf("link_title", at + title.start..at + title.end));
                }
                out.push(node_to("link_reference_definition", parts, lines[i].next));
                i += 1;
                continue;
            }
            // `[label]:` with the destination on the next line.
            // (The destination line may be indented any amount.)
            if let Some(label) = label_only(t)
                && let Some(next) = lines.get(i + 1)
                && let (_, skip) = self.indent(*next)
                && let (nt, nat) = (
                    &self.src[next.content + skip..next.end],
                    next.content + skip,
                )
                && let Some((dest, title)) = destination(nt, 0)
            {
                let mut parts = label_part(label);
                parts.push(leaf("link_destination", nat + dest.start..nat + dest.end));
                if let Some(title) = title {
                    parts.push(leaf("link_title", nat + title.start..nat + title.end));
                }
                out.push(node_to("link_reference_definition", parts, next.next));
                i += 2;
                continue;
            }
            break;
        }
        if i >= lines.len() || self.blank(lines[i]) {
            return i;
        }
        // A pipe table: a header line, then a delimiter row of as many cells.
        if let Some(next) = lines.get(i + 1)
            && let Some(cells) = delimiter_row(self.text(*next))
            && row_cells(self.text(lines[i])) == cells
            && self.text(lines[i]).contains('|')
        {
            return self.table(lines, i, out);
        }
        let first = i;
        let mut last = i;
        i += 1;
        while i < lines.len() {
            let l = lines[i];
            if self.blank(l) {
                break;
            }
            if let Some((t, _)) = self.lead(l)
                && let Some(level) = setext_level(t)
            {
                let at = self
                    .lead(lines[first])
                    .map_or(lines[first].content, |x| x.1);
                let content = self.paragraph_item(lines, last, at);
                let (_, uat) = self.lead(l).unwrap_or((t, l.content));
                let underline = if level == 1 {
                    "setext_h1_underline"
                } else {
                    "setext_h2_underline"
                };
                out.push(node_to(
                    "setext_heading",
                    vec![
                        content.with("heading_content"),
                        leaf(underline, uat..uat + t.trim_end().len()),
                    ],
                    l.next,
                ));
                return i + 1;
            }
            // A pipe table interrupts a paragraph (GFM).
            if self.text(l).contains('|')
                && lines
                    .get(i + 1)
                    .and_then(|n| delimiter_row(self.text(*n)))
                    .is_some_and(|cells| cells == row_cells(self.text(l)))
            {
                break;
            }
            if self.interrupts(l) {
                break;
            }
            last = i;
            i += 1;
        }
        let at = self
            .lead(lines[first])
            .map_or(lines[first].content, |x| x.1);
        out.push(self.paragraph_item(lines, last, at));
        last + 1
    }

    fn paragraph_item(&self, lines: &[Line], last: usize, at: usize) -> Item {
        let end_line = lines[last];
        let text_end = self.src[..end_line.end].trim_end().len().max(at);
        node_to(
            "paragraph",
            vec![leaf("inline", at..text_end)],
            end_line.next,
        )
    }

    fn table(&self, lines: &[Line], start: usize, out: &mut Vec<Item>) -> usize {
        let mut items = vec![self.row("pipe_table_header", lines[start], false)];
        items.push(self.row("pipe_table_delimiter_row", lines[start + 1], true));
        let mut i = start + 2;
        while i < lines.len() && !self.blank(lines[i]) && !self.interrupts(lines[i]) {
            items.push(self.row("pipe_table_row", lines[i], false));
            i += 1;
        }
        let end = lines[i - 1].next;
        out.push(node_to("pipe_table", items, end));
        i
    }

    fn row(&self, kind: &'static str, l: Line, delimiter: bool) -> Item {
        let (_, bytes) = self.indent(l);
        let start = l.content + bytes;
        let text = &self.src[start..l.end];
        let end = start + text.trim_end().len();
        let mut items = Vec::new();
        let cell_kind = if delimiter {
            "pipe_table_delimiter_cell"
        } else {
            "pipe_table_cell"
        };
        let pipes = pipes(&self.src[start..end]);
        let mut cell_from = start;
        let push_cell = |from: usize, to: usize, items: &mut Vec<Item>| {
            let seg = &self.src[from..to];
            let lead = seg.len() - seg.trim_start().len();
            let s = from + lead;
            let e = if delimiter { s + seg.trim().len() } else { to };
            if e > s {
                items.push(leaf(cell_kind, s..e));
            } else if !delimiter && to > from {
                // A blank cell (`| |`) is a cell, over its whitespace.
                items.push(leaf(cell_kind, from..to));
            }
        };
        for p in pipes.iter().map(|p| start + p) {
            if p > cell_from || (p == cell_from && p != start) {
                push_cell(cell_from, p, &mut items);
            }
            items.push(punct("|", p..p + 1));
            cell_from = p + 1;
        }
        if cell_from < end {
            push_cell(cell_from, end, &mut items);
        }
        node(kind, items)
    }
}

/// The byte offsets of the unescaped `|`s in `text`, outside code spans.
fn pipes(text: &str) -> Vec<usize> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'|' => {
                out.push(i);
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}

/// The number of cells in a table row.
fn row_cells(text: &str) -> usize {
    let t = text.trim();
    let p = pipes(t);
    let mut n = p.len() + 1;
    if t.starts_with('|') {
        n -= 1;
    }
    if t.ends_with('|') && t.len() > 1 && !t.ends_with("\\|") {
        n -= 1;
    }
    n
}

/// The cell count of a delimiter row (`|---|:-:|`), or `None`.
fn delimiter_row(text: &str) -> Option<usize> {
    let t = text.trim();
    if t.is_empty() || !t.contains('-') {
        return None;
    }
    let inner = t.strip_prefix('|').unwrap_or(t);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    let cells: Vec<&str> = inner.split('|').collect();
    let ok = cells.iter().all(|c| {
        let c = c.trim();
        let c = c.strip_prefix(':').unwrap_or(c);
        let c = c.strip_suffix(':').unwrap_or(c);
        !c.is_empty() && c.bytes().all(|b| b == b'-')
    });
    // A lone `---` is a setext underline or a break, not a table row.
    (ok && (t.contains('|') || cells.len() > 1)).then_some(cells.len())
}

fn atx_level(t: &str) -> Option<usize> {
    let n = t.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&n) {
        return None;
    }
    match t.as_bytes().get(n) {
        None | Some(b' ' | b'\t') => Some(n),
        _ => None,
    }
}

fn setext_level(t: &str) -> Option<u8> {
    let t = t.trim_end();
    if !t.is_empty() && t.bytes().all(|b| b == b'=') {
        return Some(1);
    }
    if !t.is_empty() && t.bytes().all(|b| b == b'-') {
        return Some(2);
    }
    None
}

fn thematic_break(t: &str) -> bool {
    let t = t.trim_end();
    let Some(c) = t.chars().next().filter(|c| matches!(c, '*' | '-' | '_')) else {
        return false;
    };
    t.chars().filter(|x| *x == c).count() >= 3 && t.chars().all(|x| x == c || x == ' ' || x == '\t')
}

/// A list marker: its kind, width, and what makes two items one list.
struct Marker {
    kind: &'static str,
    width: usize,
    /// The bullet character, or the ordered delimiter.
    delimiter: char,
    ordered: bool,
    /// Whether it may interrupt a paragraph (CommonMark: a bullet with
    /// content, or an ordered list starting at 1).
    interrupts: bool,
}

impl Marker {
    fn same_list(&self, other: &Marker) -> bool {
        self.ordered == other.ordered && self.delimiter == other.delimiter
    }
}

fn list_marker(t: &str) -> Option<Marker> {
    let b = t.as_bytes();
    let first = *b.first()?;
    let followed = |n: usize| matches!(b.get(n), None | Some(b' ' | b'\t'));
    let has_content = |n: usize| !t[n..].trim().is_empty();
    if matches!(first, b'-' | b'+' | b'*') && followed(1) {
        let kind = match first {
            b'-' => "list_marker_minus",
            b'+' => "list_marker_plus",
            _ => "list_marker_star",
        };
        return Some(Marker {
            kind,
            width: 1,
            delimiter: first as char,
            ordered: false,
            interrupts: has_content(1),
        });
    }
    let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
    if (1..=9).contains(&digits)
        && let Some(&d @ (b'.' | b')')) = b.get(digits)
        && followed(digits + 1)
    {
        return Some(Marker {
            kind: if d == b'.' {
                "list_marker_dot"
            } else {
                "list_marker_parenthesis"
            },
            width: digits + 1,
            delimiter: d as char,
            ordered: true,
            interrupts: &t[..digits] == "1" && has_content(digits + 1),
        });
    }
    None
}

const BLOCK_TAGS: &[&str] = &[
    "address",
    "article",
    "aside",
    "base",
    "basefont",
    "blockquote",
    "body",
    "caption",
    "center",
    "col",
    "colgroup",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "frame",
    "frameset",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hr",
    "html",
    "iframe",
    "legend",
    "li",
    "link",
    "main",
    "menu",
    "menuitem",
    "nav",
    "noframes",
    "ol",
    "optgroup",
    "option",
    "p",
    "param",
    "search",
    "section",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "title",
    "tr",
    "track",
    "ul",
];

/// The CommonMark HTML block type `t` starts, if any.
fn html_start(t: &str, in_paragraph: bool) -> Option<u8> {
    if !t.starts_with('<') {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    for tag in ["script", "pre", "style", "textarea"] {
        if let Some(rest) = lower.strip_prefix(&format!("<{tag}"))
            && (rest.is_empty() || rest.starts_with([' ', '\t', '>']))
        {
            return Some(1);
        }
    }
    if t.starts_with("<!--") {
        return Some(2);
    }
    if t.starts_with("<?") {
        return Some(3);
    }
    if t.starts_with("<![CDATA[") {
        return Some(5);
    }
    if t.starts_with("<!") && t.as_bytes().get(2).is_some_and(u8::is_ascii_alphabetic) {
        return Some(4);
    }
    let name_start = if t.starts_with("</") { 2 } else { 1 };
    let name: String = lower[name_start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    let after = &lower[name_start + name.len()..];
    if BLOCK_TAGS.contains(&name.as_str())
        && (after.is_empty() || after.starts_with([' ', '\t', '>']) || after.starts_with("/>"))
    {
        return Some(6);
    }
    // Any complete tag alone on its line; it cannot interrupt a paragraph.
    if !in_paragraph && !name.is_empty() && t.trim_end().ends_with('>') && !t.contains('\n') {
        let inner = t.trim_end();
        if !inner[1..inner.len() - 1].contains(['<', '>']) {
            return Some(7);
        }
    }
    None
}

struct Definition {
    label: std::ops::Range<usize>,
    dest: std::ops::Range<usize>,
    title: Option<std::ops::Range<usize>>,
}

/// `[label]: destination "title"` on one line, offsets within `t`.
fn link_definition(t: &str) -> Option<Definition> {
    let b = t.as_bytes();
    if b.first() != Some(&b'[') {
        return None;
    }
    let mut i = 1;
    while i < b.len() && b[i] != b']' {
        if b[i] == b'[' {
            return None;
        }
        if b[i] == b'\\' {
            i += 1;
        }
        i += 1;
    }
    if i >= b.len() || i == 1 || b.get(i + 1) != Some(&b':') {
        return None;
    }
    let label = 0..i + 1;
    let (dest, title) = destination(t, i + 2)?;
    Some(Definition { label, dest, title })
}

/// `[label]:` and nothing more on the line: the label, whose destination
/// is on the next line.
fn label_only(t: &str) -> Option<std::ops::Range<usize>> {
    let close = t.find("]:")?;
    (t.starts_with('[')
        && close > 1
        && !t[1..close].contains(['[', ']'])
        && t[close + 2..].trim().is_empty())
    .then_some(0..close + 1)
}

/// A definition's destination and title in `t` from `j`, offsets within
/// `t`; nothing else may follow on the line.
fn destination(
    t: &str,
    mut j: usize,
) -> Option<(std::ops::Range<usize>, Option<std::ops::Range<usize>>)> {
    let b = t.as_bytes();
    while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
        j += 1;
    }
    if j >= b.len() {
        return None;
    }
    // `<…>` is kept with its brackets, as the destination is written.
    let dest = if b[j] == b'<' {
        let close = t[j..].find('>')? + j;
        j..close + 1
    } else {
        let s = j;
        let mut depth = 0i32;
        while j < b.len() && !b[j].is_ascii_whitespace() {
            match b[j] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            j += 1;
        }
        if depth != 0 {
            return None;
        }
        s..j
    };
    let mut k = dest.end;
    let ws = k;
    while k < b.len() && (b[k] == b' ' || b[k] == b'\t') {
        k += 1;
    }
    if k >= b.len() {
        return Some((dest, None));
    }
    if k == ws {
        return None;
    }
    let close = match b[k] {
        b'"' => '"',
        b'\'' => '\'',
        b'(' => ')',
        _ => return None,
    };
    let end = t[k + 1..].find(close)? + k + 1;
    if !t[end + 1..].trim().is_empty() {
        return None;
    }
    Some((dest, Some(k..end + 1)))
}
