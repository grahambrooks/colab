//! C#'s tokens: words (identifiers and keywords alike; the parser reads
//! keywords by their text, since most of C#'s are contextual), numbers by
//! literal kind, strings, characters, verbatim and raw strings in their
//! parts, interpolated strings in the pieces tree-sitter-c-sharp's scanner
//! cuts them into, operators, and preprocessor directives. `>` is always
//! one token, so `>>` can close two type-argument lists; the expression
//! parser joins adjacent ones into `>>`, `>=`, `>>=`, `>>>` and `>>>=`.
//! Comments are extras, placed after parsing; so are the directives other
//! than `#if` / `#elif` / `#else` / `#endif`, which the parser takes out of
//! the token stream and builds as nodes of their own.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum L {
    Name,
    Op(&'static str),
    Int,
    Real,
    /// A string's or character's quote (`"` / `'`).
    Quote,
    /// `string_literal_content`.
    StrContent,
    /// `character_literal_content`.
    CharContent,
    Escape,
    /// `string_literal_encoding` (`u8` after a string).
    Encoding,
    Verbatim,
    RawStart,
    RawContent,
    RawEnd,
    /// `u8` after a raw string: no node of its own.
    RawSuffix,
    /// `$`, `$@`, `@$`, `$$` …
    InterpStart,
    /// An interpolated string's quote: `"` (anonymous) or, in a raw one,
    /// `interpolation_quote`.
    InterpQuote {
        raw: bool,
    },
    /// `string_content`.
    InterpContent,
    InterpOpen,
    InterpClose,
    /// A format clause's text after its `:`: no node of its own.
    Format,
    /// `#if`, `#elif`, `#else`, `#endif`, `#region`, `#pragma`, …: the
    /// directive's kind (its text may hold spaces after the `#`).
    Directive(&'static str),
    /// `preproc_arg`.
    PreprocArg,
    /// The newline ending a directive's line: in the directive's node,
    /// no token of its own.
    LineEnd,
    Shebang,
    Bad,
    Eof,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Lx {
    pub k: L,
    pub start: usize,
    pub end: usize,
}

/// A comment, placed after parsing.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Extra {
    pub start: usize,
    pub end: usize,
}

const OPS: &[&str] = &[
    "??=", "<<=", "=>", "==", "!=", "<=", "<<", "&&", "||", "++", "--", "+=", "-=", "*=", "/=",
    "%=", "&=", "|=", "^=", "??", "::", "->", "..", "+", "-", "*", "/", "%", "&", "|", "^", "!",
    "~", "<", ">", "=", "?", ":", ";", ",", ".", "(", ")", "[", "]", "{", "}",
];

const DIRECTIVES: &[(&str, &str)] = &[
    ("if", "#if"),
    ("elif", "#elif"),
    ("else", "#else"),
    ("endif", "#endif"),
    ("region", "#region"),
    ("endregion", "#endregion"),
    ("pragma", "#pragma"),
    ("nullable", "#nullable"),
    ("line", "#line"),
    ("error", "#error"),
    ("warning", "#warning"),
    ("define", "#define"),
    ("undef", "#undef"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum StrKind {
    Regular,
    Verbatim,
    Raw,
}

/// An interpolated string being lexed.
struct Frame {
    dollars: usize,
    kind: StrKind,
    quotes: usize,
    /// In a hole (`{…}`), and its bracket depth.
    hole: bool,
    depth: usize,
}

pub(crate) struct Lexed {
    pub tokens: Vec<Lx>,
    pub extras: Vec<Extra>,
    pub errors: bool,
}

pub(crate) fn lex(src: &str) -> Lexed {
    let mut lx = Lexer {
        src,
        b: src.as_bytes(),
        at: 0,
        out: Vec::new(),
        extras: Vec::new(),
        errors: false,
        frames: Vec::new(),
        directive: false,
    };
    if src.starts_with('\u{feff}') {
        lx.at = 3;
    }
    if src[lx.at..].starts_with("#!") {
        let end = src[lx.at..].find('\n').map_or(src.len(), |i| lx.at + i);
        lx.push(L::Shebang, lx.at, end);
        lx.at = end;
    }
    lx.run();
    lx.out.push(Lx {
        k: L::Eof,
        start: src.len(),
        end: src.len(),
    });
    Lexed {
        tokens: lx.out,
        extras: lx.extras,
        errors: lx.errors,
    }
}

struct Lexer<'s> {
    src: &'s str,
    b: &'s [u8],
    at: usize,
    out: Vec<Lx>,
    extras: Vec<Extra>,
    errors: bool,
    frames: Vec<Frame>,
    /// On a directive's line, whose newline is a token.
    directive: bool,
}

fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

fn is_ident_continue(c: char) -> bool {
    c == '_' || c.is_alphanumeric() || (!c.is_ascii() && !c.is_whitespace() && c.is_alphanumeric())
}

impl Lexer<'_> {
    fn byte(&self, i: usize) -> u8 {
        self.b.get(i).copied().unwrap_or(0)
    }

    fn char_at(&self, i: usize) -> Option<char> {
        self.src.get(i..).and_then(|s| s.chars().next())
    }

    fn push(&mut self, k: L, start: usize, end: usize) {
        if k == L::Bad {
            self.errors = true;
        }
        self.out.push(Lx { k, start, end });
    }

    fn run(&mut self) {
        while self.at < self.b.len() {
            if let Some(f) = self.frames.last()
                && !f.hole
            {
                self.string_piece();
                continue;
            }
            self.code_token();
        }
    }

    /// Whether only spaces and tabs come before `at` on its line.
    fn line_start(&self, at: usize) -> bool {
        let mut i = at;
        let first = if self.src.starts_with('\u{feff}') {
            3
        } else {
            0
        };
        while i > first {
            match self.b[i - 1] {
                b' ' | b'\t' | b'\x0c' | b'\x0b' => i -= 1,
                b'\n' => return true,
                b'\r' => return true,
                _ => return false,
            }
        }
        true
    }

    fn code_token(&mut self) {
        let c = self.byte(self.at);
        let start = self.at;
        match c {
            b'\n' => {
                if self.directive {
                    // tree-sitter's line end takes the run of newlines.
                    self.directive = false;
                    let n = self.run_of(start, b'\n');
                    self.push(L::LineEnd, start, start + n);
                    self.at += n;
                    return;
                }
                self.at += 1;
            }
            b' ' | b'\t' | b'\r' | b'\x0b' | b'\x0c' => self.at += 1,
            b'/' if self.byte(start + 1) == b'/' => {
                let mut i = start + 2;
                while i < self.b.len() && self.b[i] != b'\n' && self.b[i] != b'\r' {
                    i += 1;
                }
                self.extras.push(Extra { start, end: i });
                self.at = i;
            }
            b'/' if self.byte(start + 1) == b'*' => {
                let end = self.src[start + 2..]
                    .find("*/")
                    .map_or(self.b.len(), |i| start + 2 + i + 2);
                if end == self.b.len() && !self.src[start..].ends_with("*/") {
                    self.errors = true;
                }
                self.extras.push(Extra { start, end });
                self.at = end;
            }
            b'#' if self.frames.is_empty() && self.line_start(start) => self.directive_line(),
            b'"' => {
                let run = self.run_of(start, b'"');
                if run >= 3 {
                    self.raw_string(start, run);
                } else {
                    self.string_literal(start);
                }
            }
            b'\'' => self.char_literal(start),
            b'$' => self.interp_start(start),
            b'@' if self.byte(start + 1) == b'"' => self.verbatim(start),
            b'@' if self.byte(start + 1) == b'$' => self.interp_start(start),
            b'0'..=b'9' => self.number(start),
            b'.' if self.byte(start + 1).is_ascii_digit() => self.number(start),
            _ => {
                let ch = self.char_at(start).unwrap_or('\0');
                if ch == '@' || is_ident_start(ch) || ch == '\\' {
                    self.word(start);
                    return;
                }
                if ch.is_whitespace() {
                    self.at += ch.len_utf8();
                    return;
                }
                if let Some(op) = OPS.iter().find(|o| self.src[start..].starts_with(**o)) {
                    self.at = start + op.len();
                    self.op(op, start);
                    return;
                }
                self.at = start + ch.len_utf8();
                self.push(L::Bad, start, self.at);
            }
        }
    }

    /// An operator, with an interpolation hole's bookkeeping.
    fn op(&mut self, op: &'static str, start: usize) {
        let run = self.run_of(start, b'}');
        if let Some(f) = self.frames.last_mut() {
            match op {
                "(" | "[" | "{" => f.depth += 1,
                ")" | "]" if f.depth > 0 => f.depth -= 1,
                "}" if f.depth > 0 => f.depth -= 1,
                "}" => {
                    // The hole's close: `dollars` braces.
                    let need = f.dollars;
                    if run >= need {
                        let end = start + need;
                        f.hole = false;
                        self.at = end;
                        self.push(L::InterpClose, start, end);
                        return;
                    }
                }
                ":" if f.depth == 0 => {
                    // A format clause: `:` and its text, to `}` or `"`.
                    self.push(L::Op(":"), start, start + 1);
                    let mut i = start + 1;
                    while i < self.b.len() && self.b[i] != b'}' && self.b[i] != b'"' {
                        i += 1;
                    }
                    if i > start + 1 {
                        self.push(L::Format, start + 1, i);
                    }
                    self.at = i;
                    return;
                }
                _ => {}
            }
        }
        self.push(L::Op(op), start, start + op.len());
    }

    fn run_of(&self, at: usize, c: u8) -> usize {
        self.b[at..].iter().take_while(|&&x| x == c).count()
    }

    fn word(&mut self, start: usize) {
        let mut i = start;
        if self.byte(i) == b'@' {
            i += 1;
        }
        let mut first = true;
        while let Some(c) = self.char_at(i) {
            if c == '\\' && matches!(self.byte(i + 1), b'u' | b'U') {
                let n = if self.byte(i + 1) == b'u' { 4 } else { 8 };
                if self.b.len() >= i + 2 + n
                    && self.b[i + 2..i + 2 + n].iter().all(u8::is_ascii_hexdigit)
                {
                    i += 2 + n;
                    first = false;
                    continue;
                }
                break;
            }
            let ok = if first {
                is_ident_start(c)
            } else {
                is_ident_continue(c) || is_combining(c)
            };
            if !ok {
                break;
            }
            i += c.len_utf8();
            first = false;
        }
        if first {
            self.at = start + 1;
            self.push(L::Bad, start, self.at);
            return;
        }
        self.at = i;
        self.push(L::Name, start, i);
    }

    fn number(&mut self, start: usize) {
        let b = self.b;
        let mut i = start;
        let digits = |mut i: usize, hex: bool| {
            while i < b.len()
                && (b[i] == b'_'
                    || if hex {
                        b[i].is_ascii_hexdigit()
                    } else {
                        b[i].is_ascii_digit()
                    })
            {
                i += 1;
            }
            i
        };
        if b[i] == b'0' && matches!(self.byte(i + 1), b'x' | b'X' | b'b' | b'B') {
            i = digits(i + 2, true);
            i = int_suffix(b, i);
            self.at = i;
            self.push(L::Int, start, i);
            return;
        }
        let mut real = false;
        if b[i] != b'.' {
            i = digits(i, false);
        }
        if self.byte(i) == b'.' && self.byte(i + 1).is_ascii_digit() {
            real = true;
            i = digits(i + 1, false);
        }
        if matches!(self.byte(i), b'e' | b'E') {
            let mut k = i + 1;
            if matches!(self.byte(k), b'+' | b'-') {
                k += 1;
            }
            if self.byte(k).is_ascii_digit() {
                real = true;
                i = digits(k, false);
            }
        }
        if matches!(self.byte(i), b'f' | b'F' | b'd' | b'D' | b'm' | b'M') {
            real = true;
            i += 1;
        } else if !real {
            i = int_suffix(b, i);
        }
        self.at = i;
        self.push(if real { L::Real } else { L::Int }, start, i);
    }

    /// An escape sequence at `at` (a `\`): its end, if it is one.
    fn escape_end(&self, at: usize) -> Option<usize> {
        let hex = |from: usize, min: usize, max: usize| {
            let n = self.b[from.min(self.b.len())..]
                .iter()
                .take(max)
                .take_while(|c| c.is_ascii_hexdigit())
                .count();
            (n >= min).then_some(from + n)
        };
        match self.byte(at + 1) {
            b'x' => hex(at + 2, 1, 4),
            b'u' => hex(at + 2, 4, 4),
            b'U' => hex(at + 2, 8, 8),
            b'a' | b'b' | b'e' | b'f' | b'n' | b'r' | b't' | b'v' | b'\'' | b'"' | b'\\' | b'?'
            | b'0' => Some(at + 2),
            _ => None,
        }
    }

    fn escape(&mut self, at: usize) {
        if let Some(end) = self.escape_end(at) {
            self.push(L::Escape, at, end);
            self.at = end;
        } else {
            let end = at + 1 + self.char_at(at + 1).map_or(0, char::len_utf8);
            self.push(L::Bad, at, end);
            self.at = end;
        }
    }

    fn string_literal(&mut self, start: usize) {
        self.push(L::Quote, start, start + 1);
        self.at = start + 1;
        loop {
            let i = self.at;
            match self.byte(i) {
                0 if i >= self.b.len() => return,
                b'"' => {
                    self.push(L::Quote, i, i + 1);
                    self.at = i + 1;
                    if matches!(self.byte(i + 1), b'u' | b'U') && self.byte(i + 2) == b'8' {
                        self.push(L::Encoding, i + 1, i + 3);
                        self.at = i + 3;
                    }
                    return;
                }
                b'\\' => self.escape(i),
                b'\n' => {
                    // An unclosed string: the parser misses its quote.
                    self.errors = true;
                    return;
                }
                _ => {
                    let mut k = i;
                    while k < self.b.len() && !matches!(self.b[k], b'"' | b'\\' | b'\n') {
                        k += 1;
                    }
                    self.push(L::StrContent, i, k);
                    self.at = k;
                }
            }
        }
    }

    fn char_literal(&mut self, start: usize) {
        self.push(L::Quote, start, start + 1);
        let i = start + 1;
        match self.byte(i) {
            b'\\' => self.escape(i),
            b'\'' | b'\n' => {
                self.at = i;
            }
            _ => {
                let n = self.char_at(i).map_or(1, char::len_utf8);
                self.push(L::CharContent, i, i + n);
                self.at = i + n;
            }
        }
        if self.byte(self.at) == b'\'' {
            self.push(L::Quote, self.at, self.at + 1);
            self.at += 1;
        } else {
            self.errors = true;
        }
    }

    fn verbatim(&mut self, start: usize) {
        let mut i = start + 2;
        loop {
            if i >= self.b.len() {
                self.errors = true;
                break;
            }
            if self.b[i] == b'"' {
                if self.byte(i + 1) == b'"' {
                    i += 2;
                    continue;
                }
                i += 1;
                break;
            }
            i += 1;
        }
        if matches!(self.byte(i), b'u' | b'U') && self.byte(i + 1) == b'8' {
            i += 2;
        }
        self.at = i;
        self.push(L::Verbatim, start, i);
    }

    fn raw_string(&mut self, start: usize, run: usize) {
        self.push(L::RawStart, start, start + run);
        let mut i = start + run;
        let content = i;
        loop {
            if i >= self.b.len() {
                self.push(L::RawContent, content, i);
                self.errors = true;
                self.at = i;
                return;
            }
            if self.b[i] == b'"' {
                let n = self.run_of(i, b'"');
                if n == run {
                    self.push(L::RawContent, content, i);
                    self.push(L::RawEnd, i, i + n);
                    i += n;
                    break;
                }
                i += n;
                continue;
            }
            i += 1;
        }
        if matches!(self.byte(i), b'u' | b'U') && self.byte(i + 1) == b'8' {
            self.push(L::RawSuffix, i, i + 2);
            i += 2;
        }
        self.at = i;
    }

    /// `$"`, `$@"`, `@$"`, `$$"""`: the start and its quote, and a frame.
    fn interp_start(&mut self, start: usize) {
        let mut i = start;
        let mut verbatim = false;
        if self.byte(i) == b'@' {
            verbatim = true;
            i += 1;
        }
        let dollars = self.run_of(i, b'$');
        i += dollars;
        if dollars == 0 || !matches!(self.byte(i), b'"' | b'@') {
            // Not a string: `$` alone is no token of C#'s.
            self.at = start + 1;
            self.push(L::Bad, start, self.at);
            return;
        }
        if self.byte(i) == b'@' {
            verbatim = true;
            i += 1;
        }
        self.push(L::InterpStart, start, i);
        let run = self.run_of(i, b'"');
        let (kind, quotes) = if !verbatim && run >= 3 {
            (StrKind::Raw, run)
        } else if verbatim {
            (StrKind::Verbatim, 1)
        } else {
            (StrKind::Regular, 1)
        };
        if run == 0 {
            self.at = i;
            self.errors = true;
            return;
        }
        self.push(
            L::InterpQuote {
                raw: kind == StrKind::Raw,
            },
            i,
            i + quotes,
        );
        self.at = i + quotes;
        self.frames.push(Frame {
            dollars,
            kind,
            quotes,
            hole: false,
            depth: 0,
        });
    }

    /// The next piece of an interpolated string's text: its end quote, a
    /// hole's open brace, an escape, or `string_content`, cut where
    /// tree-sitter-c-sharp's scanner cuts it.
    fn string_piece(&mut self) {
        let Some(f) = self.frames.last() else { return };
        let (kind, dollars, quotes) = (f.kind, f.dollars, f.quotes);
        let i = self.at;
        let c = self.byte(i);
        // The end quote.
        if c == b'"' {
            match kind {
                StrKind::Raw => {
                    let n = self.run_of(i, b'"');
                    if n == quotes {
                        self.push(L::InterpQuote { raw: true }, i, i + n);
                        self.at = i + n;
                        self.frames.pop();
                        return;
                    }
                }
                StrKind::Verbatim if self.byte(i + 1) == b'"' => {}
                _ => {
                    self.push(L::InterpQuote { raw: false }, i, i + 1);
                    self.at = i + 1;
                    self.frames.pop();
                    return;
                }
            }
        }
        if kind == StrKind::Regular {
            if c == b'\\' {
                self.escape(i);
                return;
            }
            if c == b'\n' {
                // A regular string cannot hold a line break.
                self.errors = true;
                self.frames.pop();
                return;
            }
        }
        // A hole: exactly `dollars` braces, not followed by another.
        let mut braces = 0;
        if c == b'{' {
            braces = self.run_of(i, b'{').min(dollars);
            if braces == dollars && self.byte(i + braces) != b'{' {
                self.push(L::InterpOpen, i, i + braces);
                self.at = i + braces;
                if let Some(f) = self.frames.last_mut() {
                    f.hole = true;
                    f.depth = 0;
                }
                return;
            }
        }
        // Content, as the scanner reads it.
        let mut k = i + braces;
        let mut advanced = braces;
        let mut taken = braces > 0;
        if c == b'"' {
            // A verbatim `""` or a raw run of too few or many quotes.
            let n = if kind == StrKind::Verbatim {
                2
            } else {
                self.run_of(i, b'"')
            };
            k = i + n;
            taken = true;
            advanced = 0;
        }
        while k < self.b.len() {
            let ch = self.b[k];
            match kind {
                StrKind::Regular if matches!(ch, b'\\' | b'\n' | b'"') => break,
                StrKind::Verbatim if ch == b'"' => {
                    if self.byte(k + 1) == b'"' {
                        k += 2;
                        taken = true;
                        advanced = 0;
                        continue;
                    }
                    break;
                }
                StrKind::Raw if ch == b'"' => {
                    let n = self.run_of(k, b'"');
                    if n == quotes {
                        break;
                    }
                    k += n;
                    taken = true;
                    advanced = 0;
                    continue;
                }
                _ => {}
            }
            if ch == b'{' {
                if advanced == 0 {
                    break;
                }
                if kind != StrKind::Raw {
                    // An escaped `{{`: take the second and count again.
                    k += 1;
                    advanced = 0;
                    taken = true;
                    continue;
                }
                k += 1;
                taken = true;
                continue;
            }
            advanced = 0;
            k += self.char_at(k).map_or(1, char::len_utf8);
            taken = true;
        }
        if !taken {
            // Nothing the scanner would take: an error, one byte on.
            let n = self.char_at(i).map_or(1, char::len_utf8);
            self.push(L::Bad, i, i + n);
            self.at = i + n;
            if i + n >= self.b.len() {
                self.frames.pop();
            }
            return;
        }
        self.push(L::InterpContent, i, k);
        self.at = k;
    }

    /// A directive: `#`, spaces, its name, and the rest of its line.
    fn directive_line(&mut self) {
        let start = self.at;
        let mut i = start + 1;
        while matches!(self.byte(i), b' ' | b'\t') {
            i += 1;
        }
        let name_start = i;
        while self.byte(i).is_ascii_alphanumeric() || self.byte(i) == b'_' {
            i += 1;
        }
        let name = &self.src[name_start..i];
        let Some(&(_, kind)) = DIRECTIVES.iter().find(|(n, _)| *n == name) else {
            // `#:` and the like: unknown here.
            let end = self.src[start..]
                .find('\n')
                .map_or(self.b.len(), |e| start + e);
            self.push(L::Bad, start, end);
            self.at = end;
            return;
        };
        self.push(L::Directive(kind), start, i);
        self.at = i;
        match kind {
            "#else" | "#endif" => {}
            "#region" | "#endregion" | "#error" | "#warning" | "#define" | "#undef" => {
                self.directive = true;
                self.preproc_arg();
            }
            _ => self.directive = true,
        }
    }

    /// The rest of a directive's line as `preproc_arg`, then its newline.
    fn preproc_arg(&mut self) {
        let mut i = self.at;
        while matches!(self.byte(i), b' ' | b'\t' | b'\r' | b'\x0b' | b'\x0c') {
            i += 1;
        }
        self.at = i;
        if i >= self.b.len() || self.b[i] == b'\n' {
            return;
        }
        // A comment wins over the argument it would start.
        if self.byte(i) == b'/' && matches!(self.byte(i + 1), b'/' | b'*') {
            return;
        }
        let start = i;
        while i < self.b.len() {
            match self.b[i] {
                b'\n' => break,
                b'/' if self.byte(i + 1) == b'*' => break,
                b'/' => i += 2,
                b'\\' if self.byte(i + 1) == b'\n' => i += 2,
                b'\\' if self.byte(i + 1) == b'\r' && self.byte(i + 2) == b'\n' => i += 3,
                _ => i += 1,
            }
        }
        let i = i.min(self.b.len());
        self.push(L::PreprocArg, start, i);
        self.at = i;
    }
}

fn int_suffix(b: &[u8], mut i: usize) -> usize {
    let at = |i: usize| b.get(i).copied().unwrap_or(0);
    match at(i) {
        b'u' | b'U' => {
            i += 1;
            if matches!(at(i), b'l' | b'L') {
                i += 1;
            }
        }
        b'l' | b'L' => {
            i += 1;
            if matches!(at(i), b'u' | b'U') {
                i += 1;
            }
        }
        _ => {}
    }
    i
}

fn is_combining(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x200C | 0x200D)
}
