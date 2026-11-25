//! Structural patterns over a [`Tree`]: code with metavariables, matched
//! node for node: colab's own matcher (it replaced ast-grep's in 2026-11).
//!
//! A pattern is a parsed fragment ([`Pattern::from_node`]): each node whose
//! text is a metavariable becomes one (`$X` one node, `$$$X` a run of
//! siblings, `$_` / `$$$` the same without a binding), every other node
//! must be met by a candidate node of the same kind with children that
//! match in order, and every token by one with the same text.
//!
//! The rules are the ones colab needs for rewriting, built in rather than
//! checked afterwards:
//!
//! - **Every named child is accounted for.** A candidate's named children
//!   are each matched by a pattern child, or absorbed by a `$$$`. Nothing
//!   trails unseen, so a rewrite of `if $C { $$$B }` never drops an `else`.
//! - **A syntax error is never skipped.** An `ERROR` node is named, so only
//!   a metavariable can take it, and a rewrite copies a capture verbatim;
//!   a match never silently loses text the parser could not read.
//! - **Anonymous tokens are treated alike everywhere.** A candidate's
//!   keyword or punctuation token may be passed over only where the pattern
//!   has nothing to put against it (`static` before a method a pattern
//!   names by `$M`, a trailing `,`), and every token passed over is listed
//!   in the [`Match`], so a rewrite can keep it. A token the pattern spells
//!   must be there.
//! - **`$$$` backtracks.** A sequence takes as few siblings as it can and
//!   more when what follows does not match, so `f($$$A, $L)` finds the last
//!   argument wherever it is.
//! - **A repeated metavariable means the same code**, compared token by
//!   token with layout and comments set aside.
//!
//! Comments (`extra` nodes) and trivia are invisible to matching on both
//! sides. Matching recurses only as deep as the *pattern*, never the file.

use crate::{Kind, Node, Tree};

/// What a metavariable stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetaVar {
    /// `$X` (one named node) or `$$X` (one node, named or not), bound to
    /// `name`; `None` for `$_`, which binds nothing.
    One {
        name: Option<String>,
        named_only: bool,
    },
    /// `$$$X`: zero or more siblings, bound to `name`; `None` for `$$$`
    /// and `$$$_`.
    Many { name: Option<String> },
}

/// `text` as a metavariable written with `expando` (`$`, or `µ` where `$`
/// cannot start an identifier and patterns are [`expand`]ed): `$X`, `$$X`,
/// `$_`, `$$$X`, `$$$`. Names are upper case, digits and `_`, starting
/// with a letter or `_`. A statement holding only a sequence (`$$$B;`) is
/// the sequence.
pub fn meta_variable(text: &str, expando: char) -> Option<MetaVar> {
    let text = text.trim();
    if let Some(stripped) = text.strip_suffix(';')
        && let Some(var @ MetaVar::Many { .. }) = meta_variable(stripped, expando)
    {
        return Some(var);
    }
    let valid_first = |c: char| c.is_ascii_uppercase() || c == '_';
    let valid = |c: char| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_';
    let name_of = |name: &str| -> Option<Option<String>> {
        if !name.starts_with(valid_first) || !name.chars().all(valid) {
            return None;
        }
        Some((!name.starts_with('_')).then(|| name.to_string()))
    };
    let ellipsis: String = std::iter::repeat_n(expando, 3).collect();
    if text == ellipsis {
        return Some(MetaVar::Many { name: None });
    }
    if let Some(name) = text.strip_prefix(&ellipsis) {
        return Some(MetaVar::Many {
            name: name_of(name)?,
        });
    }
    let rest = text.strip_prefix(expando)?;
    let (name, named_only) = match rest.strip_prefix(expando) {
        Some(name) => (name, false),
        None => (rest, true),
    };
    Some(MetaVar::One {
        name: name_of(name)?,
        named_only,
    })
}

/// `query` with each metavariable's `$`s replaced by `expando`, so the
/// pattern parses where `$` cannot start an identifier. A `$` that starts
/// no metavariable (`"$5"`, a PHP `$x`) is left alone.
pub fn expand(query: &str, expando: char) -> String {
    if expando == '$' {
        return query.to_string();
    }
    let is_var_start = |c: char| c.is_ascii_uppercase() || c == '_';
    let chars: Vec<char> = query.chars().collect();
    let mut out = String::with_capacity(query.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' {
            let run = chars[i..].iter().take_while(|&&c| c == '$').count();
            let named = chars.get(i + run).is_some_and(|&c| is_var_start(c));
            if (run <= 3 && named) || (run == 3 && !named) {
                out.extend(std::iter::repeat_n(expando, run));
                i += run;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// One node of a compiled pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternNode {
    /// A token the candidate must repeat: same text, and the same kind when
    /// it is named (`x` the identifier, not `x` the type).
    Token {
        kind: Kind,
        text: String,
        named: bool,
    },
    /// A node of `kind` whose significant children match `children` in
    /// order.
    Node {
        kind: Kind,
        children: Vec<PatternNode>,
    },
    /// A metavariable.
    Var(MetaVar),
}

/// A language's rules for an item's modifiers (`public static`, `pub`,
/// `static inline`, `@Deprecated`), which a pattern usually leaves out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rules {
    /// Kinds a pattern may leave out: where a pattern node spells none of
    /// them, the candidate's leading children of these kinds are taken as
    /// given (and listed in [`Match::absorbed`], for a rewrite to keep), so
    /// `fn f()` matches `pub fn f()`. Where it spells one, they are matched
    /// as written.
    pub absorb: Vec<Kind>,
    /// Kinds matched token for token, passing over nothing: a modifiers
    /// node the pattern spells, so `public void f()` does not match
    /// `public static void f()`.
    pub exact: Vec<Kind>,
    /// Modifiers a pattern may leave out even when it spells others
    /// (JavaScript's `static` and `async`, TypeScript's `private`): a
    /// candidate child of these kinds with no goal for it is passed over
    /// and listed in [`Match::absorbed`], so `private f()` matches
    /// `private static async f()` and a rewrite can keep the rest.
    pub loose: Vec<Kind>,
}

/// A compiled pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    root: PatternNode,
    rules: Rules,
}

/// Whether matching passes over `node`: trivia, comments, and missing
/// tokens (zero-width, standing for nothing written; a pattern's come
/// from its placeholders). tree-sitter also marks some recovered `ERROR`
/// nodes as extra; an error with text is never invisible, or a match
/// would skip text it cannot read.
fn invisible(node: Node<'_>) -> bool {
    node.is_trivia() || node.is_missing() || (node.is_extra() && !node.is_error())
}

/// The children matching sees.
fn significant<'t>(node: Node<'t>) -> Vec<Node<'t>> {
    node.children().filter(|c| !invisible(*c)).collect()
}

fn compile(node: Node<'_>, expando: char) -> PatternNode {
    if let Some(var) = meta_variable(node.text(), expando) {
        return PatternNode::Var(var);
    }
    if node.is_token() {
        return PatternNode::Token {
            kind: node.kind(),
            text: node.text().to_string(),
            named: node.is_named(),
        };
    }
    PatternNode::Node {
        kind: node.kind(),
        children: significant(node)
            .into_iter()
            .map(|c| compile(c, expando))
            .collect(),
    }
}

impl Pattern {
    /// A pattern from its nodes, as a parser or a test composes it.
    pub fn new(root: PatternNode) -> Self {
        Self {
            root,
            rules: Rules::default(),
        }
    }

    /// The pattern `node` spells: a node of a tree parsed from the pattern
    /// text (expanded with `expando`), usually the one node that spans it.
    pub fn from_node(node: Node<'_>, expando: char) -> Self {
        Self::new(compile(node, expando))
    }

    /// The pattern matched under a language's modifier `rules`.
    #[must_use]
    pub fn with_rules(mut self, rules: Rules) -> Self {
        self.rules = rules;
        self
    }

    pub fn rules(&self) -> &Rules {
        &self.rules
    }

    pub fn root(&self) -> &PatternNode {
        &self.root
    }

    /// The kind every match has, unless the pattern is a lone
    /// metavariable, which matches any node.
    pub fn kind(&self) -> Option<Kind> {
        match &self.root {
            PatternNode::Token { kind, .. } | PatternNode::Node { kind, .. } => Some(*kind),
            PatternNode::Var(_) => None,
        }
    }

    /// The match at `candidate`, if the pattern matches it.
    pub fn matches<'t>(&self, candidate: Node<'t>) -> Option<Match<'t>> {
        if invisible(candidate) {
            return None;
        }
        let mut state = State::default();
        let goal = match &self.root {
            // A lone `$$$X` at the root stands for one node.
            PatternNode::Var(MetaVar::Many { name }) => {
                if let Some(name) = name {
                    state.bind_many(name, vec![candidate])?;
                }
                return Some(state.into_match(candidate));
            }
            goal => goal,
        };
        match_node(goal, candidate, &mut state, &self.rules).then(|| state.into_match(candidate))
    }

    /// Every match in `tree`, in document order: an outer match before
    /// those inside it. Of nodes over exactly the same text (rust-analyzer's
    /// `path_expr › path › … › ident`), only the outermost that matches is
    /// a match: they are one place, and one's binding would hold the other.
    pub fn find_all<'t>(&self, tree: &'t Tree) -> Vec<Match<'t>> {
        let kind = self.kind();
        let mut out: Vec<Match<'t>> = Vec::new();
        for m in tree
            .nodes()
            .filter(|n| kind.is_none_or(|k| n.kind() == k))
            .filter_map(|n| self.matches(n))
        {
            // Document order puts a node's same-range descendants right
            // after it, so the previous match is the one to compare.
            if out
                .last()
                .is_some_and(|last| last.node.range() == m.node.range())
            {
                continue;
            }
            out.push(m);
        }
        out
    }
}

/// What a metavariable was bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding<'t> {
    One(Node<'t>),
    Many(Vec<Node<'t>>),
}

impl Binding<'_> {
    /// The source text the binding covers (empty for an empty sequence).
    pub fn text(&self) -> &str {
        match (self, self.range()) {
            (Binding::One(n), _) => n.text(),
            (Binding::Many(nodes), Some(range)) => &nodes[0].tree().text()[range],
            _ => "",
        }
    }

    /// The byte range it covers; `None` for an empty sequence. A sequence
    /// takes the comments directly before its first node and after its
    /// last (back to the previous sibling the match used, on to the next):
    /// matching passes over them, but a rewrite must not drop them, and a
    /// doc comment belongs to the item after it. Whitespace at its end is
    /// left out.
    pub fn range(&self) -> Option<std::ops::Range<usize>> {
        match self {
            Binding::One(n) => Some(n.range()),
            Binding::Many(nodes) => {
                let comment = |n: &Node<'_>| n.is_extra() && !n.is_error();
                let (first, last) = (*nodes.first()?, *nodes.last()?);
                let mut start = first.start();
                let mut prev = first.prev_sibling();
                while let Some(p) = prev {
                    if comment(&p) {
                        start = p.start();
                    } else if !p.is_trivia() {
                        break;
                    }
                    prev = p.prev_sibling();
                }
                let mut end = last.end();
                let mut next = last.next_sibling();
                while let Some(n) = next {
                    if comment(&n) {
                        end = n.end();
                    } else if !n.is_trivia() {
                        break;
                    }
                    next = n.next_sibling();
                }
                // Whitespace ending it is layout, not code: Go's statement
                // list owns the newline after its last statement.
                let text = first.tree().text();
                Some(start..start + text[start..end].trim_end().len())
            }
        }
    }
}

/// A pattern matched at a node.
#[derive(Debug, Clone)]
pub struct Match<'t> {
    pub node: Node<'t>,
    /// Each named metavariable's binding, in the order first bound.
    pub bindings: Vec<(String, Binding<'t>)>,
    /// The candidate's anonymous tokens the pattern had nothing for
    /// (`static`, a trailing `,`), in document order.
    pub skipped: Vec<Node<'t>>,
    /// The candidate's modifiers the pattern left out ([`Rules::absorb`]),
    /// in document order.
    pub absorbed: Vec<Node<'t>>,
}

impl<'t> Match<'t> {
    pub fn get(&self, name: &str) -> Option<&Binding<'t>> {
        self.bindings
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, b)| b)
    }
}

#[derive(Debug, Clone, Default)]
struct State<'t> {
    bindings: Vec<(String, Binding<'t>)>,
    skipped: Vec<Node<'t>>,
    absorbed: Vec<Node<'t>>,
}

impl<'t> State<'t> {
    fn into_match(mut self, node: Node<'t>) -> Match<'t> {
        self.skipped.sort_by_key(Node::id);
        self.absorbed.sort_by_key(Node::id);
        Match {
            node,
            bindings: self.bindings,
            skipped: self.skipped,
            absorbed: self.absorbed,
        }
    }

    fn bind_one(&mut self, name: &str, node: Node<'t>) -> Option<()> {
        match self.bindings.iter().find(|(n, _)| n == name) {
            Some((_, Binding::One(bound))) => same_code(&[*bound], &[node]).then_some(()),
            Some((_, Binding::Many(_))) => None,
            None => {
                self.bindings.push((name.to_string(), Binding::One(node)));
                Some(())
            }
        }
    }

    fn bind_many(&mut self, name: &str, nodes: Vec<Node<'t>>) -> Option<()> {
        match self.bindings.iter().find(|(n, _)| n == name) {
            Some((_, Binding::Many(bound))) => same_code(bound, &nodes).then_some(()),
            Some((_, Binding::One(_))) => None,
            None => {
                self.bindings.push((name.to_string(), Binding::Many(nodes)));
                Some(())
            }
        }
    }
}

/// The significant tokens of `nodes`, in order: none from trivia or from
/// inside a comment (Rust's block comments have tokens of their own).
fn code_tokens<'t>(nodes: &[Node<'t>]) -> Vec<&'t str> {
    let mut out = Vec::new();
    for node in nodes {
        let mut hidden_until = 0;
        for n in node.descendants() {
            if n.start() < hidden_until {
                continue;
            }
            if invisible(n) {
                hidden_until = n.end().max(n.start() + 1);
                continue;
            }
            if n.is_token() {
                out.push(n.text());
            }
        }
    }
    out
}

/// Whether two runs of nodes are the same code, layout and comments aside.
fn same_code(a: &[Node<'_>], b: &[Node<'_>]) -> bool {
    code_tokens(a) == code_tokens(b)
}

/// Whether `goals` spell a node or token of one of `kinds`.
fn spells(goals: &[PatternNode], kinds: &[Kind]) -> bool {
    goals.iter().any(|g| match g {
        PatternNode::Token { kind, .. } | PatternNode::Node { kind, .. } => kinds.contains(kind),
        PatternNode::Var(_) => false,
    })
}

fn match_node<'t>(
    goal: &PatternNode,
    cand: Node<'t>,
    state: &mut State<'t>,
    rules: &Rules,
) -> bool {
    match goal {
        PatternNode::Token { kind, text, named } => {
            cand.text() == text && (!named || cand.kind() == *kind)
        }
        PatternNode::Var(MetaVar::One { name, named_only }) => {
            if *named_only && !cand.is_named() {
                return false;
            }
            match name {
                Some(name) => state.bind_one(name, cand).is_some(),
                None => true,
            }
        }
        // A sequence among one node's worth of candidates: one node.
        PatternNode::Var(MetaVar::Many { name }) => match name {
            Some(name) => state.bind_many(name, vec![cand]).is_some(),
            None => true,
        },
        PatternNode::Node { kind, children } => {
            if cand.kind() != *kind || cand.is_token() {
                return false;
            }
            let mut cands = significant(cand);
            // Modifiers the pattern leaves out are taken as given. Loose
            // kinds among them (rust-analyzer's `attr` before a `pub`) stay
            // for the children's match, which passes over them.
            if !rules.absorb.is_empty() && !spells(children, &rules.absorb) {
                let leading = cands
                    .iter()
                    .take_while(|c| {
                        rules.absorb.contains(&c.kind()) || rules.loose.contains(&c.kind())
                    })
                    .count();
                let mut kept = Vec::new();
                for c in cands.drain(..leading) {
                    if rules.absorb.contains(&c.kind()) {
                        state.absorbed.push(c);
                    } else {
                        kept.push(c);
                    }
                }
                cands.splice(0..0, kept);
            }
            let exact = rules.exact.contains(kind);
            match_children(children, &cands, state, rules, exact)
        }
    }
}

/// `goals` against `cands`, all of them: each candidate is matched by a
/// goal, absorbed by a sequence, or (an anonymous token only, and not in
/// an `exact` node) skipped.
fn match_children<'t>(
    goals: &[PatternNode],
    cands: &[Node<'t>],
    state: &mut State<'t>,
    rules: &Rules,
    exact: bool,
) -> bool {
    match_children_from(goals, cands, state, rules, exact, true)
}

/// [`match_children`]; `skip_first` false right after a sequence, whose
/// end is where the next goal matches: a token there is the sequence's
/// to take, never one to pass over (a TypeScript member's `;` stays in
/// `{ $$$M }`'s `$$$M`, or a rewrite would drop it).
fn match_children_from<'t>(
    goals: &[PatternNode],
    cands: &[Node<'t>],
    state: &mut State<'t>,
    rules: &Rules,
    exact: bool,
    skip_first: bool,
) -> bool {
    let skippable = |c: &Node<'_>| !exact && c.is_token() && !c.is_named();
    let Some((goal, rest)) = goals.split_first() else {
        // Only anonymous tokens may be left over.
        if cands.iter().all(skippable) {
            state.skipped.extend_from_slice(cands);
            return true;
        }
        return false;
    };
    if let PatternNode::Var(MetaVar::Many { name }) = goal {
        // Last, a sequence takes everything left: nothing after it could
        // claim a token, so none is passed over.
        if rest.is_empty() {
            if let Some(name) = name {
                return state.bind_many(name, cands.to_vec()).is_some();
            }
            return true;
        }
        // Otherwise as few siblings as will do, then more.
        for take in 0..=cands.len() {
            let mut attempt = state.clone();
            if let Some(name) = name
                && attempt.bind_many(name, cands[..take].to_vec()).is_none()
            {
                continue;
            }
            if match_children_from(rest, &cands[take..], &mut attempt, rules, exact, false) {
                *state = attempt;
                return true;
            }
        }
        return false;
    }
    let Some((&cand, later)) = cands.split_first() else {
        return false;
    };
    let mut attempt = state.clone();
    if match_node(goal, cand, &mut attempt, rules)
        && match_children(rest, later, &mut attempt, rules, exact)
    {
        *state = attempt;
        return true;
    }
    // A keyword or punctuation the pattern has nothing for here, or a
    // modifier it may leave out.
    let loose = !exact && rules.loose.contains(&cand.kind());
    if skip_first && (skippable(&cand) || loose) {
        let mut attempt = state.clone();
        if loose {
            attempt.absorbed.push(cand);
        } else {
            attempt.skipped.push(cand);
        }
        if match_children(goals, later, &mut attempt, rules, exact) {
            *state = attempt;
            return true;
        }
    }
    false
}
