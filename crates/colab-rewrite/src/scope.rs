//! `where inside '<pattern>'` on a rule that is not a pattern: a
//! syntactic scope.
//!
//! ```text
//! match rust::symbol "config" where inside 'impl Loader { $$$B }' { replace "settings" }
//! match go::call "log.Print" where not inside 'func Test$N($$$P) { $$$B }' { replace_call "slog.Info($args)" }
//! ```
//!
//! [`Scoped`] runs the rule as usual, then keeps only the changes that
//! fall where the constraints allow: inside some match of each `inside`
//! pattern, and outside every match of each `not inside` one. The scope
//! patterns are ordinary `<lang>::pattern`s, compiled by the backend, so
//! a scope is a function, an `impl`, a class, or any shape a pattern can
//! name. It is still syntactic: nothing resolves names, so a shadowing
//! local inside the scope is renamed too. Changes are compared token by
//! token (identifier runs, and each other character), so a rename is kept
//! or dropped one identifier at a time.

use std::fmt;
use std::ops::Range;
use std::path::Path;

use colab_core::{Constraint, Operation, Test};

/// A compiled scope: the pattern's matches, and whether a change must be
/// inside one (`inside`) or outside all of them (`not inside`).
pub struct Scope {
    pub pattern: Box<dyn Operation>,
    pub negated: bool,
}

/// A rule narrowed to the regions its scopes allow.
pub struct Scoped {
    inner: Box<dyn Operation>,
    scopes: Vec<Scope>,
    constraints: Vec<Constraint>,
}

impl fmt::Debug for Scoped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scoped")
            .field("inner", &self.inner)
            .field("constraints", &self.constraints)
            .finish()
    }
}

impl fmt::Display for Scoped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `<lang>::<module> "<target>" where <constraints> -> …`, so the
        // namespace still leads (`colab:ignore go::symbol` keeps working).
        let inner = self.inner.to_string();
        let where_ = self
            .constraints
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        match inner.find(" -> ") {
            Some(at) => write!(f, "{} where {where_}{}", &inner[..at], &inner[at..]),
            None => write!(f, "{inner} where {where_}"),
        }
    }
}

/// Why a constraint cannot narrow a rule that is not a pattern.
pub fn unsupported(lang: &str, module: &str, constraint: &Constraint) -> String {
    format!(
        "`{lang}::{module}` takes only `inside '<pattern>'` and `not inside '<pattern>'` in its `where` \
         clause (the scope it may change); `{constraint}` needs a metavariable, which only \
         `{lang}::pattern` binds"
    )
}

impl Scoped {
    /// `inner` narrowed by `constraints`, each an `inside` test whose
    /// pattern `compile` turns into an operation that can `locate` its
    /// matches (the backend's own `<lang>::pattern`).
    pub fn new<F>(
        lang: &str,
        module: &str,
        inner: Box<dyn Operation>,
        constraints: &[Constraint],
        mut compile: F,
    ) -> colab_core::Result<Self>
    where
        F: FnMut(&str) -> colab_core::Result<Box<dyn Operation>>,
    {
        let mut scopes = Vec::new();
        for constraint in constraints {
            let Test::Inside(pattern) = &constraint.test else {
                return Err(colab_core::Error::UnsupportedOperation(unsupported(
                    lang, module, constraint,
                )));
            };
            scopes.push(Scope {
                pattern: compile(pattern)?,
                negated: constraint.negated,
            });
        }
        Ok(Self {
            inner,
            scopes,
            constraints: constraints.to_vec(),
        })
    }

    /// The byte ranges each scope pattern matches in `source`.
    fn regions(&self, source: &str) -> Vec<Vec<Range<usize>>> {
        self.scopes
            .iter()
            .map(|scope| {
                scope
                    .pattern
                    .locate(source)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|l| l.start..l.end)
                    .collect()
            })
            .collect()
    }

    /// Whether a change over `span` of the original is allowed.
    fn allows(&self, regions: &[Vec<Range<usize>>], span: &Range<usize>) -> bool {
        self.scopes.iter().zip(regions).all(|(scope, ranges)| {
            let inside = ranges
                .iter()
                .any(|r| r.start <= span.start && span.end <= r.end);
            inside != scope.negated
        })
    }

    /// `after` (what the rule made of `before`) with only the allowed
    /// changes kept.
    fn narrow(&self, before: &str, after: &str) -> String {
        if before == after {
            return after.to_string();
        }
        let regions = self.regions(before);
        let old = tokens(before);
        let new = tokens(after);
        let ops = similar::capture_diff_slices(similar::Algorithm::Myers, &old, &new);
        // Byte offset of each old token.
        let mut starts = Vec::with_capacity(old.len() + 1);
        let mut at = 0;
        for t in &old {
            starts.push(at);
            at += t.len();
        }
        starts.push(at);
        let mut out = String::with_capacity(after.len());
        for op in ops {
            let (o, n) = (op.old_range(), op.new_range());
            if matches!(op.tag(), similar::DiffTag::Equal) {
                out.extend(old[o].iter().copied());
                continue;
            }
            let span = starts[o.start]..starts[o.end];
            if self.allows(&regions, &span) {
                out.extend(new[n].iter().copied());
            } else {
                out.extend(old[o].iter().copied());
            }
        }
        out
    }
}

/// `text` as tokens: maximal runs of identifier characters, and every
/// other character on its own.
fn tokens(text: &str) -> Vec<&str> {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        match (start, word(c)) {
            (None, true) => start = Some(i),
            (Some(s), false) => {
                out.push(&text[s..i]);
                start = None;
                out.push(&text[i..i + c.len_utf8()]);
            }
            (None, false) => out.push(&text[i..i + c.len_utf8()]),
            (Some(_), true) => {}
        }
    }
    if let Some(s) = start {
        out.push(&text[s..]);
    }
    out
}

impl Operation for Scoped {
    fn is_file_relevant(&self, path: &Path) -> bool {
        self.inner.is_file_relevant(path)
    }

    fn apply(&self, source: &str) -> String {
        self.narrow(source, &self.inner.apply(source))
    }

    /// The rule's own: a scope only removes changes.
    fn prefilter(&self) -> Option<&str> {
        self.inner.prefilter()
    }

    fn near_miss_target(&self) -> Option<&str> {
        self.inner.near_miss_target()
    }

    fn observed_targets(&self, source: &str) -> Vec<String> {
        self.inner.observed_targets(source)
    }

    fn conflicts(&self, before: &str, after: &str) -> Vec<String> {
        self.inner.conflicts(before, after)
    }

    fn why_unchanged(&self, source: &str) -> Option<String> {
        let inner = self.inner.apply(source);
        if inner != source && self.narrow(source, &inner) == source {
            return Some(
                "it matched only outside the scope its `where` clause allows, so nothing was changed"
                    .into(),
            );
        }
        self.inner.why_unchanged(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renames every `old` to `new`; locates every `{ … }` block.
    #[derive(Debug)]
    struct Fake(&'static str);

    impl fmt::Display for Fake {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "x::symbol \"old\" -> \"new\"")
        }
    }

    impl Operation for Fake {
        fn is_file_relevant(&self, _: &Path) -> bool {
            true
        }
        fn apply(&self, source: &str) -> String {
            source.replace("old", "new")
        }
        fn locate(&self, source: &str) -> Option<Vec<colab_core::find::Located>> {
            let (open, close) = (self.0.chars().next()?, self.0.chars().nth(1)?);
            let mut out = Vec::new();
            let mut from = 0;
            while let Some(s) = source[from..].find(open) {
                let start = from + s;
                let end = start + source[start..].find(close)? + 1;
                out.push(colab_core::find::Located::at(
                    source,
                    start,
                    end,
                    Vec::new(),
                ));
                from = end;
            }
            Some(out)
        }
    }

    fn scoped(constraints: &[(&'static str, bool)]) -> Scoped {
        let cs: Vec<Constraint> = constraints
            .iter()
            .map(|(p, negated)| Constraint {
                negated: *negated,
                test: Test::Inside(p.to_string()),
            })
            .collect();
        Scoped::new("x", "symbol", Box::new(Fake("")), &cs, |p| {
            Ok(Box::new(Fake(if p == "braces" { "{}" } else { "[]" })))
        })
        .unwrap()
    }

    #[test]
    fn tokens_split_identifiers_from_everything_else() {
        assert_eq!(tokens("a.old_x(b)"), ["a", ".", "old_x", "(", "b", ")"]);
    }

    #[test]
    fn inside_keeps_changes_in_scope_and_not_inside_the_rest() {
        let src = "old { old old } old [old]";
        assert_eq!(
            scoped(&[("braces", false)]).apply(src),
            "old { new new } old [old]"
        );
        assert_eq!(
            scoped(&[("braces", true)]).apply(src),
            "new { old old } new [new]"
        );
        // Constraints combine: in braces' complement and in brackets.
        assert_eq!(
            scoped(&[("braces", true), ("brackets", false)]).apply(src),
            "old { old old } old [new]"
        );
        let op = scoped(&[("braces", false)]);
        assert_eq!(
            op.to_string(),
            "x::symbol \"old\" where inside 'braces' -> \"new\""
        );
        assert!(
            op.why_unchanged("old [old]")
                .unwrap()
                .contains("outside the scope")
        );
    }

    #[test]
    fn only_inside_narrows_a_symbol() {
        let c = Constraint {
            negated: false,
            test: Test::Has("x".into()),
        };
        let e = Scoped::new("go", "symbol", Box::new(Fake("")), &[c], |_| unreachable!())
            .unwrap_err()
            .to_string();
        assert!(e.contains("takes only `inside"), "{e}");
    }
}
