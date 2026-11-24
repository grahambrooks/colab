//! Abstract syntax tree produced by the LALRPOP grammar.
//!
//! These types mirror the codemod DSL one-to-one and have no runtime
//! behaviour. They are lowered into the executable
//! [`crate::Refactoring`] IR by [`crate::compiler`].

/// A complete codemod script: `refactor "<name>" { <item>* }`.
///
/// Items are either match blocks or `include` directives, intermixed
/// in source order. The compiler expands includes recursively so the
/// runtime IR sees a flat list of match clauses.
#[derive(PartialEq, Debug)]
pub struct Command {
    pub refactor_name: String,
    pub items: Vec<Item>,
}

/// One element inside a `refactor "..." { … }` block.
// A script holds a handful of items, parsed once and lowered at once;
// boxing `Match` would buy nothing but indirection at every use.
#[allow(clippy::large_enum_variant)]
#[derive(PartialEq, Debug)]
pub enum Item {
    Match(Match),
    Include(String),
    /// `pattern <name> = '<code>'`: a pattern constraints refer to by name.
    Pattern {
        name: String,
        text: String,
        span: Span,
    },
    /// `scan <lang>::pattern '<p>' … as <name>`: a read-only phase.
    Scan(Scan),
    /// `then { … }`: items that run after everything before them.
    Then(Vec<Item>),
    /// `rewriter <name> = '<pattern>' -> '<template>'`: what a `let`'s
    /// `rewrite(<name>)` step does to each match inside a binding.
    Rewriter {
        name: String,
        pattern: String,
        template: String,
        span: Span,
    },
}

/// One `match <namespace> "<target>" [in "<glob>"] [where …] { <action> <import clause>* }`
/// block.
/// A script may contain any number of these; they are applied in source
/// order.
#[derive(PartialEq, Debug)]
pub struct Match {
    pub namespace: Namespace,
    pub match_string: String,
    /// Optional path glob from an `in "<glob>"` clause. `None` means the
    /// rule applies wherever its language backend considers a file
    /// relevant.
    pub scope: Option<String>,
    /// `in fences` / `and fences`: the rule also, or only, runs inside
    /// Markdown code fences of its language.
    pub fences: Option<Fences>,
    /// `and prose`: a `symbol` rename also renames whole-word matches in
    /// comments and string contents.
    pub prose: bool,
    /// `message "…"`: what the rule's finding means, for reports.
    pub message: Option<String>,
    /// `severity error | warning | info`; `None` means `error`.
    pub severity: Option<colab_core::Severity>,
    /// `id "<name>"`: what `--only` / `--off` / `--severity` select it by.
    pub id: Option<String>,
    /// `url "<link>"`: where the rule's rationale lives, for SARIF and
    /// GitHub annotations.
    pub url: Option<String>,
    /// Constraints from a `where` clause, in order. Empty without one.
    pub constraints: Vec<colab_core::Constraint>,
    /// `let $NEW = $FROM | step …` clauses before the action, in order.
    pub lets: Vec<Let>,
    pub action: Action,
    /// `requires import` / `drops import … when unused` clauses after the
    /// action, in order.
    pub imports: Vec<ImportClause>,
    /// Byte range of the whole block in the script, `match` to `}`.
    /// Lets the LSP place a diagnostic on the rule that caused it.
    pub span: Span,
}

/// Where a rule runs relative to Markdown code fences.
#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub enum Fences {
    /// `in fences`: only inside fences, in Markdown files.
    Only,
    /// `and fences`: the rule's own files, and fences.
    Also,
}

/// One location clause, before they are folded into a [`Match`].
#[derive(PartialEq, Eq, Debug, Clone)]
pub enum Location {
    Glob(String),
    Fences(Fences),
    Prose,
}

/// A `match` header's `where` list and the location clauses written after
/// it (`where … in "src/**"`), before they are folded into a [`Match`].
#[derive(PartialEq, Eq, Debug, Clone)]
pub struct WhereTail {
    pub constraints: Vec<colab_core::Constraint>,
    pub locations: Vec<Location>,
}

/// `let $NEW = $FROM | step | …`: a value derived from a binding for a
/// `<lang>::pattern` `replace` template. Steps are `(name, args)` as
/// written; the compiler checks them ([`colab_core::Transform::parse`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Let {
    pub var: colab_core::Var,
    pub from: colab_core::Var,
    pub steps: Vec<(String, Vec<colab_core::TransformArg>)>,
}

impl std::fmt::Display for Let {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "let {} = {}", self.var, self.from)?;
        for (name, args) in &self.steps {
            write!(f, " | {name}")?;
            if !args.is_empty() {
                let args: Vec<String> = args.iter().map(ToString::to_string).collect();
                write!(f, "({})", args.join(", "))?;
            }
        }
        Ok(())
    }
}

/// `scan <namespace> '<pattern>' [in "<glob>"] [where …] as <name>`.
#[derive(Debug, PartialEq)]
pub struct Scan {
    pub namespace: Namespace,
    pub pattern: String,
    pub scope: Option<String>,
    pub constraints: Vec<colab_core::Constraint>,
    pub name: String,
    pub span: Span,
}

/// One clause after a rule's action.
#[derive(PartialEq, Eq, Debug, Clone)]
pub enum Clause {
    Import(ImportClause),
    Message(String),
    Severity(String),
    Id(String),
    Url(String),
}

/// The clauses after an action: imports in order, then at most one of
/// each of the others.
#[derive(Debug, Default)]
pub(crate) struct Clauses {
    pub imports: Vec<ImportClause>,
    pub message: Option<String>,
    pub severity: Option<colab_core::Severity>,
    pub id: Option<String>,
    pub url: Option<String>,
}

/// `true` for a rule id: letters, digits, and `-` `_` `.` `/`, not empty.
/// The text of a double-quoted DSL string: `\"` is `"`, `\\` is `\`,
/// `\n` a line break and `\t` a tab, as in JSON. Any other backslash is
/// kept as written (`"\d+"` stays `\d+`), so text with stray backslashes
/// means what it shows. Single-quoted and raw strings are never unescaped.
pub fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// `text` as a double-quoted DSL string, escaped so it reads back as
/// itself (the inverse of [`unescape`]).
pub fn quote_double(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
}

pub(crate) fn clauses(clauses: Vec<Clause>) -> Result<Clauses, &'static str> {
    let mut out = Clauses::default();
    let (imports, message, severity) = (&mut out.imports, &mut out.message, &mut out.severity);
    for clause in clauses {
        match clause {
            Clause::Id(id) if out.id.is_none() => {
                if !valid_id(&id) {
                    return Err(
                        "an `id` is letters, digits and `-` `_` `.` `/`, e.g. `id \"no-dbg\"`",
                    );
                }
                out.id = Some(id);
            }
            Clause::Id(_) => return Err("a rule takes one `id`"),
            Clause::Url(url) if out.url.is_none() => out.url = Some(url),
            Clause::Url(_) => return Err("a rule takes one `url`"),
            Clause::Import(i) => imports.push(i),
            Clause::Message(m) if message.is_none() => *message = Some(m),
            Clause::Message(_) => return Err("a rule takes one `message`"),
            Clause::Severity(s) if severity.is_none() => {
                *severity = Some(
                    colab_core::Severity::parse(&s)
                        .ok_or("`severity` is `error`, `warning` or `info`")?,
                );
            }
            Clause::Severity(_) => return Err("a rule takes one `severity`"),
        }
    }
    Ok(out)
}

/// Fold a rule's location clauses, refusing one given twice.
pub(crate) fn locations(
    locations: Vec<Location>,
) -> Result<(Option<String>, Option<Fences>, bool), &'static str> {
    let (mut glob, mut fences, mut prose) = (None, None, false);
    for location in locations {
        match location {
            Location::Prose if !prose => prose = true,
            Location::Prose => return Err("a rule takes one `and prose` clause"),
            Location::Glob(g) if glob.is_none() => glob = Some(g),
            Location::Fences(f) if fences.is_none() => fences = Some(f),
            Location::Glob(_) => return Err("a rule takes one `in \"<glob>\"` clause"),
            Location::Fences(_) => {
                return Err("a rule takes one of `in fences` or `and fences`, once");
            }
        }
    }
    Ok((glob, fences, prose))
}

/// An import clause on a rule, run only on the files the rule changed.
#[derive(PartialEq, Eq, Debug, Clone)]
pub enum ImportClause {
    /// `requires import "<x>"`: ensure the import is present.
    Requires(String),
    /// `drops import "<x>" when unused`: delete the import if nothing in
    /// the file still uses it.
    DropsUnused(String),
}

impl std::fmt::Display for ImportClause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportClause::Requires(x) => write!(f, "requires import \"{x}\""),
            ImportClause::DropsUnused(x) => write!(f, "drops import \"{x}\" when unused"),
        }
    }
}

/// A byte range in the script source.
#[derive(PartialEq, Eq, Debug, Clone, Copy, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

/// A target namespace such as `go::import`.
#[derive(PartialEq, Debug)]
pub struct Namespace {
    pub lang: String,
    pub module: String,
}

/// The action performed when a match succeeds.
#[derive(PartialEq, Debug)]
pub enum Action {
    /// Replace the matched value with the supplied string.
    Replace(String),
    /// Remove the matched element entirely.
    Delete,
    /// Idempotently add the matched element if it is missing. The
    /// `match_string` is the value to ensure exists; no other input
    /// is needed.
    Ensure,
    /// Rewrite a matched call expression using a string template.
    /// Placeholders in the template:
    ///
    /// - `$1`, `$2`, … — 1-indexed positional argument from the
    ///   matched call.
    /// - `$args` — the original argument list, comma-joined.
    /// - `$func` — the matched function name (verbatim).
    /// - `$$` — a literal `$`.
    ReplaceCall(String),
    /// Give the matched key or element this value, creating it (and any
    /// missing parents) when absent and overwriting it when different.
    /// Config-file backends only; the value is in the file's own syntax.
    Set(String),
    /// Add the matched key or element with this value only when it is
    /// absent; an existing value is left alone.
    Insert(String),
    /// `create "<path>" '<template>'`: a new file from each match, the
    /// path relative to the matched file's directory. Patterns only.
    Create { path: String, template: String },
}

impl Action {
    /// The DSL keyword this action is written with.
    pub fn keyword(&self) -> &'static str {
        match self {
            Action::Replace(_) => "replace",
            Action::Delete => "delete",
            Action::Ensure => "ensure",
            Action::ReplaceCall(_) => "replace_call",
            Action::Set(_) => "set",
            Action::Insert(_) => "insert",
            Action::Create { .. } => "create",
        }
    }
}
