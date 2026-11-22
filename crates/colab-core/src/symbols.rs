//! Declarations by name path, for `colab edit --symbol Config/load`.
//!
//! A file's declarations form a tree (a `struct`'s `impl`, its methods);
//! a name path is the names on the way down, `/`-separated, and it names
//! a declaration when it is the end of that declaration's full path: so
//! `load` finds every `load`, and `Config/load` only the one in `Config`.
//! More than one is an error that lists them. The declarations come from
//! the language server's `documentSymbol` when it answers, else from
//! tree-sitter (`colab_rewrite::declarations`); either way an edit is plain
//! text at a byte range, reported like any other run.

use std::ops::Range;

/// One declaration and those inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    /// Its name as the path segment it is: `load`, or `Config` for an
    /// `impl Config` block.
    pub name: String,
    /// What it is (`function_item`, `method`, …), for messages.
    pub kind: String,
    /// The bytes it spans, doc comments and attributes included.
    pub range: Range<usize>,
    pub children: Vec<Declaration>,
}

/// A declaration found by a name path: its full path and itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found<'a> {
    pub path: Vec<&'a str>,
    pub declaration: &'a Declaration,
}

/// Every declaration in `tree`, depth first, with its full path.
pub fn flatten(tree: &[Declaration]) -> Vec<Found<'_>> {
    fn walk<'a>(decls: &'a [Declaration], prefix: &mut Vec<&'a str>, out: &mut Vec<Found<'a>>) {
        for d in decls {
            prefix.push(&d.name);
            out.push(Found {
                path: prefix.clone(),
                declaration: d,
            });
            walk(&d.children, prefix, out);
            prefix.pop();
        }
    }
    let mut out = Vec::new();
    walk(tree, &mut Vec::new(), &mut out);
    out
}

/// The one declaration `name_path` names in `tree`, or why there is not
/// one: none (with the nearest names), or several (with each, and its
/// line in `source`).
pub fn resolve<'a>(
    tree: &'a [Declaration],
    name_path: &str,
    source: &str,
) -> Result<Found<'a>, String> {
    let wanted: Vec<&str> = name_path.split('/').filter(|s| !s.is_empty()).collect();
    if wanted.is_empty() {
        return Err("a name path names something: `load`, or `Config/load`".into());
    }
    let all = flatten(tree);
    let mut hits: Vec<Found<'a>> = all
        .iter()
        .filter(|f| f.path.ends_with(&wanted))
        .cloned()
        .collect();
    // A server may list one declaration twice (a flat and a nested view).
    hits.dedup_by(|a, b| a.declaration.range == b.declaration.range);
    match hits.len() {
        1 => Ok(hits.remove(0)),
        0 => {
            let last = wanted[wanted.len() - 1];
            let names: Vec<String> = all.iter().map(|f| f.path.join("/")).collect();
            let hint =
                crate::suggest::closest(last, all.iter().map(|f| f.declaration.name.as_str()))
                    .map_or_default(|n| format!(" (did you mean `{n}`?)"));
            let listed = if names.is_empty() {
                "it has none".to_string()
            } else {
                let shown: Vec<String> = names.iter().take(20).map(|n| format!("`{n}`")).collect();
                let more = names.len().saturating_sub(20);
                let more = if more > 0 {
                    format!(" and {more} more")
                } else {
                    String::new()
                };
                format!("it has {}{more}", shown.join(", "))
            };
            Err(format!("no declaration `{name_path}`{hint}; {listed}"))
        }
        _ => {
            let listed: Vec<String> = hits
                .iter()
                .map(|f| {
                    let line = source[..f.declaration.range.start.min(source.len())]
                        .matches('\n')
                        .count()
                        + 1;
                    format!("`{}` (line {line})", f.path.join("/"))
                })
                .collect();
            Err(format!(
                "`{name_path}` names {} declarations: {}; give more of the path",
                hits.len(),
                listed.join(", ")
            ))
        }
    }
}

/// What to do at a declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolEdit {
    /// Put `text` in the declaration's place, all of it.
    ReplaceBody,
    /// Put `text` on the lines before it, and a blank line.
    InsertBefore,
    /// Put a blank line and `text` on the lines after it.
    InsertAfter,
}

/// `text` with the indentation its lines share removed.
fn dedent(text: &str) -> String {
    let text = text.trim_matches('\n');
    let common = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    text.lines()
        .map(|l| {
            if l.trim().is_empty() {
                ""
            } else {
                &l[common.min(l.len())..]
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `source` with `edit` made at `range` (a declaration's): `text` is
/// re-indented to the declaration's own indentation.
pub fn apply(source: &str, range: Range<usize>, edit: SymbolEdit, text: &str) -> String {
    let line_start = source[..range.start].rfind('\n').map_or(0, |i| i + 1);
    let indent: String = source[line_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let block = |first_indented: bool| -> String {
        dedent(text)
            .lines()
            .enumerate()
            .map(|(i, l)| {
                if l.is_empty() || (i == 0 && !first_indented) {
                    l.to_string()
                } else {
                    format!("{indent}{l}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let end = range.end.min(source.len());
    match edit {
        SymbolEdit::ReplaceBody => {
            format!(
                "{}{}{}",
                &source[..range.start],
                block(false),
                &source[end..]
            )
        }
        SymbolEdit::InsertBefore => {
            format!(
                "{}{}\n\n{}",
                &source[..line_start],
                block(true),
                &source[line_start..]
            )
        }
        SymbolEdit::InsertAfter => {
            // After the declaration's last line.
            let line_end = source[end..].find('\n').map_or(source.len(), |i| end + i);
            let (head, tail) = source.split_at(line_end);
            format!("{head}\n\n{}{tail}", block(true))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(name: &str, range: Range<usize>, children: Vec<Declaration>) -> Declaration {
        Declaration {
            name: name.into(),
            kind: "fn".into(),
            range,
            children,
        }
    }

    const SRC: &str = "impl A {\n    fn load() {}\n}\nimpl B {\n    fn load() {}\n}\nfn top() {}\n";

    /// `needle`'s range in `SRC`, from byte `from`.
    fn at(needle: &str, from: usize) -> Range<usize> {
        let start = from + SRC[from..].find(needle).unwrap();
        start..start + needle.len()
    }

    fn tree() -> Vec<Declaration> {
        let a = at("impl A {\n    fn load() {}\n}", 0);
        let b = at("impl B {\n    fn load() {}\n}", 0);
        vec![
            decl(
                "A",
                a.clone(),
                vec![decl("load", at("fn load() {}", a.start), vec![])],
            ),
            decl(
                "B",
                b.clone(),
                vec![decl("load", at("fn load() {}", b.start), vec![])],
            ),
            decl("top", at("fn top() {}", 0), vec![]),
        ]
    }

    #[test]
    fn a_name_path_is_a_suffix_of_the_full_path() {
        let t = tree();
        let b_load = at("fn load() {}", at("impl B", 0).start);
        assert_eq!(
            resolve(&t, "B/load", SRC).unwrap().declaration.range,
            b_load
        );
        assert_eq!(resolve(&t, "top", SRC).unwrap().path, ["top"]);
        let e = resolve(&t, "load", SRC).unwrap_err();
        assert!(
            e.contains("names 2 declarations") && e.contains("`A/load` (line 2)"),
            "{e}"
        );
        let e = resolve(&t, "lod", SRC).unwrap_err();
        assert!(e.contains("did you mean `load`"), "{e}");
    }

    #[test]
    fn edits_take_the_declarations_indentation() {
        let range = at("fn load() {}", 0);
        let new = apply(
            SRC,
            range.clone(),
            SymbolEdit::ReplaceBody,
            "fn load() {\n    1\n}",
        );
        assert!(
            new.starts_with("impl A {\n    fn load() {\n        1\n    }\n}"),
            "{new}"
        );
        let before = apply(
            SRC,
            range.clone(),
            SymbolEdit::InsertBefore,
            "fn first() {}",
        );
        assert!(
            before.starts_with("impl A {\n    fn first() {}\n\n    fn load() {}\n}"),
            "{before}"
        );
        let after = apply(SRC, range, SymbolEdit::InsertAfter, "  fn next() {}\n");
        assert!(
            after.starts_with("impl A {\n    fn load() {}\n\n    fn next() {}\n}"),
            "{after}"
        );
    }
}
