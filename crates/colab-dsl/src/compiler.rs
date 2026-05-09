//! Parse codemod scripts and lower them into the runtime IR.
//!
//! The grammar (see `codemod.lalrpop`) is consumed by [`parse`],
//! which returns a raw [`Command`] AST. [`compile`] /
//! [`compile_at_path`] then expand `include` directives, validate
//! the AST, and ask the supplied [`BackendRegistry`] to produce one
//! [`Operation`] per match block, yielding an executable
//! [`Refactoring`].

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use lalrpop_util::lalrpop_mod;

use crate::ast::{Action, Command, Item, Match};
use crate::model::Refactoring;
use colab_core::{BackendRegistry, Error, Operation, Result, RuleSpec};

lalrpop_mod!(grammar, "/codemod.rs");

/// Parse `text` into a raw AST without applying any semantic checks.
pub fn parse(text: &str) -> Result<Command> {
    grammar::ProgramParser::new()
        .parse(text)
        .map_err(|e| Error::Parse(e.to_string()))
}

/// Parse and validate `text`, returning an executable [`Refactoring`].
///
/// Returns [`Error::UnsupportedOperation`] when the script targets a
/// namespace or action that no backend in `backends` implements.
///
/// **`include` directives.** This entry point has no base path so
/// `include "..."` raises [`Error::Config`]. Use
/// [`compile_at_path`] for scripts that may include other files.
pub fn compile(text: &str, backends: &BackendRegistry) -> Result<Refactoring> {
    compile_inner(text, backends, None)
}

/// Read `path`, parse it, expand any `include "..."` directives
/// relative to `path`'s parent, and lower the result. The path is
/// canonicalised for cycle detection so back-and-forth includes
/// surface a clear error.
pub fn compile_at_path(path: &Path, backends: &BackendRegistry) -> Result<Refactoring> {
    let text = fs::read_to_string(path).map_err(|e| Error::io_at(path, e))?;
    compile_inner(&text, backends, Some(path))
}

fn compile_inner(
    text: &str,
    backends: &BackendRegistry,
    base_path: Option<&Path>,
) -> Result<Refactoring> {
    let command = parse(text)?;

    let mut seen: HashSet<PathBuf> = HashSet::new();
    if let Some(p) = base_path
        && let Ok(canonical) = p.canonicalize()
    {
        seen.insert(canonical);
    }

    let matches = expand_includes(command.items, base_path, &mut seen)?;

    let mut rules: Vec<Box<dyn Operation>> = Vec::with_capacity(matches.len());
    for m in matches {
        rules.push(lower_match(m, backends)?);
    }
    Ok(Refactoring {
        name: command.refactor_name,
        rules,
    })
}

/// Recursively expand `include` directives, in source order.
/// Returns the flattened list of `Match` clauses ready for lowering.
fn expand_includes(
    items: Vec<Item>,
    base_path: Option<&Path>,
    seen: &mut HashSet<PathBuf>,
) -> Result<Vec<Match>> {
    let mut out = Vec::new();
    for item in items {
        match item {
            Item::Match(m) => out.push(m),
            Item::Include(rel) => {
                let resolved = resolve_include_path(&rel, base_path)?;
                let canonical = resolved.canonicalize().map_err(|e| {
                    Error::io_at(&resolved, e)
                })?;
                if !seen.insert(canonical.clone()) {
                    return Err(Error::Config(format!(
                        "circular include: {} re-includes itself",
                        resolved.display()
                    )));
                }
                let text = fs::read_to_string(&resolved)
                    .map_err(|e| Error::io_at(&resolved, e))?;
                let inner = parse(&text)?;
                let nested = expand_includes(inner.items, Some(&resolved), seen)?;
                out.extend(nested);
            }
        }
    }
    Ok(out)
}

fn resolve_include_path(relative: &str, base_path: Option<&Path>) -> Result<PathBuf> {
    let p = Path::new(relative);
    if p.is_absolute() {
        return Ok(p.to_path_buf());
    }
    let base = base_path.ok_or_else(|| {
        Error::Config(format!(
            "cannot resolve `include \"{}\"`: no base path. Use `compile_at_path` instead of `compile` to enable includes.",
            relative
        ))
    })?;
    let parent = base.parent().unwrap_or(Path::new("."));
    Ok(parent.join(relative))
}

fn lower_match(m: Match, backends: &BackendRegistry) -> Result<Box<dyn Operation>> {
    let Match {
        namespace,
        match_string,
        action,
    } = m;
    let backend = backends.get(namespace.lang.as_str()).ok_or_else(|| {
        Error::UnsupportedOperation(format!(
            "{}::{} is not a supported namespace",
            namespace.lang, namespace.module
        ))
    })?;
    let spec = match action {
        Action::Replace(replacement) => RuleSpec::Replace {
            target: match_string,
            replacement,
        },
        Action::Delete => RuleSpec::Delete {
            target: match_string,
        },
        Action::Ensure => RuleSpec::Ensure {
            target: match_string,
        },
        Action::ReplaceCall(template) => RuleSpec::ReplaceCall {
            target: match_string,
            template,
        },
    };
    backend.build_rule(namespace.module.as_str(), spec)
}

/// Convenience: count the number of match clauses in `command`,
/// ignoring `include` items. Used by tests.
#[cfg(test)]
fn match_count(command: &Command) -> usize {
    command
        .items
        .iter()
        .filter(|i| matches!(i, Item::Match(_)))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Action, Match, Namespace};
    use colab_core::CodeTransformer;

    fn registry() -> BackendRegistry {
        let mut r = BackendRegistry::new();
        r.register(Box::new(colab_lang_go::GoBackend));
        r
    }

    #[test]
    fn parses_string_literal() {
        let result = grammar::StringLiteralParser::new()
            .parse(r#"  "Hello, World!"  "#)
            .unwrap();
        assert_eq!(result, "Hello, World!");
    }

    #[test]
    fn parses_identifier() {
        let result = grammar::IdentifierParser::new()
            .parse("HelloWorld122 ")
            .unwrap();
        assert_eq!(result, "HelloWorld122");
    }

    #[test]
    fn parses_action() {
        let result = grammar::ActionParser::new()
            .parse(r#"  replace "a.b.c" "#)
            .unwrap();
        assert_eq!(result, Action::Replace("a.b.c".to_string()));
    }

    #[test]
    fn parses_namespace() {
        let result = grammar::NamespaceParser::new()
            .parse("  go::import ")
            .unwrap();
        assert_eq!(
            result,
            Namespace {
                lang: "go".to_string(),
                module: "import".to_string(),
            }
        );
    }

    #[test]
    fn parses_match_block() {
        let result = grammar::MatchParser::new()
            .parse(r#" match  go::import "a.b.c" { replace "d.e.f" } "#)
            .unwrap();
        assert_eq!(
            result,
            Match {
                namespace: Namespace {
                    lang: "go".to_string(),
                    module: "import".to_string(),
                },
                match_string: "a.b.c".to_string(),
                action: Action::Replace("d.e.f".to_string()),
            }
        );
    }

    #[test]
    fn parses_full_program_with_single_match() {
        let command =
            parse(r#" refactor "this" { match  go::import "a.b.c" { replace "d.e.f" } } "#)
                .unwrap();

        assert_eq!(command.refactor_name, "this");
        assert_eq!(match_count(&command), 1);
        let Item::Match(m) = &command.items[0] else {
            panic!("expected Match item");
        };
        assert_eq!(m.namespace.lang, "go");
        assert_eq!(m.namespace.module, "import");
        assert_eq!(m.match_string, "a.b.c");
        assert_eq!(m.action, Action::Replace("d.e.f".to_string()));
    }

    #[test]
    fn parses_full_program_with_multiple_matches() {
        let command = parse(
            r#"
            refactor "many" {
                match go::import "a" { replace "b" }
                match go::import "c" { replace "d" }
                match go::import "e" { replace "f" }
            }
            "#,
        )
        .unwrap();

        assert_eq!(match_count(&command), 3);
    }

    #[test]
    fn parses_include_directive() {
        let command = parse(
            r#"
            refactor "with-include" {
                include "shared.codemod"
                match go::import "x" { replace "y" }
            }
            "#,
        )
        .unwrap();
        assert_eq!(command.items.len(), 2);
        assert!(matches!(command.items[0], Item::Include(ref s) if s == "shared.codemod"));
        assert!(matches!(command.items[1], Item::Match(_)));
    }

    #[test]
    fn ignores_line_comments() {
        let command = parse(
            r#"
            // top-level comment
            refactor "with-comments" {
                // before any rule
                match go::import "a" { replace "b" } // trailing comment
                // between rules
                match go::import "c" { replace "d" }
            }
            "#,
        )
        .unwrap();
        assert_eq!(command.refactor_name, "with-comments");
        assert_eq!(match_count(&command), 2);
    }

    #[test]
    fn parses_empty_refactor_block() {
        let command = parse(r#"refactor "noop" { }"#).unwrap();
        assert_eq!(command.refactor_name, "noop");
        assert!(command.items.is_empty());
    }

    #[test]
    fn compiles_go_import_replace() {
        let refactoring = compile(
            r#"refactor "rename" { match go::import "old" { replace "new" } }"#,
            &registry(),
        )
        .unwrap();
        assert_eq!(refactoring.name, "rename");
        assert_eq!(refactoring.len(), 1);
        let display = refactoring.to_string();
        assert!(
            display.contains("go::import \"old\" -> \"new\""),
            "got: {display}"
        );
    }

    #[test]
    fn compiles_multiple_rules_and_applies_in_order() {
        let refactoring = compile(
            r#"
            refactor "chain" {
                match go::import "old" { replace "mid" }
                match go::import "mid" { replace "new" }
            }
            "#,
            &registry(),
        )
        .unwrap();
        assert_eq!(refactoring.len(), 2);

        let source = r#"package main
import (
    "old"
)
"#;
        let rewritten = refactoring.apply(source);
        assert!(rewritten.contains("\"new\""), "got: {rewritten}");
        assert!(!rewritten.contains("\"old\""));
        assert!(!rewritten.contains("\"mid\""));
    }

    #[test]
    fn rejects_unsupported_namespace() {
        let err = compile(
            r#"refactor "x" { match rust::module "a" { replace "b" } }"#,
            &registry(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::UnsupportedOperation(_)));
    }

    #[test]
    fn rejects_unsupported_module_within_known_lang() {
        let err = compile(
            r#"refactor "x" { match go::nope "a" { replace "b" } }"#,
            &registry(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::UnsupportedOperation(_)));
    }

    #[test]
    fn compile_rejects_include_without_base_path() {
        let err = compile(
            r#"refactor "x" { include "other.codemod" }"#,
            &registry(),
        )
        .unwrap_err();
        assert!(
            matches!(&err, Error::Config(msg) if msg.contains("compile_at_path")),
            "got: {err}"
        );
    }

    #[test]
    fn compile_at_path_expands_include_relative_to_script() {
        let dir = std::env::temp_dir().join(format!(
            "colab-include-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let main = dir.join("main.codemod");
        let shared = dir.join("shared.codemod");
        std::fs::write(
            &main,
            r#"refactor "outer" { include "shared.codemod" match go::import "z" { replace "Z" } }"#,
        )
        .unwrap();
        std::fs::write(
            &shared,
            r#"refactor "inner" {
                match go::import "a" { replace "A" }
                match go::import "b" { replace "B" }
            }"#,
        )
        .unwrap();

        let refactoring = compile_at_path(&main, &registry()).unwrap();
        assert_eq!(refactoring.name, "outer");
        assert_eq!(refactoring.len(), 3);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn compile_at_path_detects_circular_include() {
        let dir = std::env::temp_dir().join(format!(
            "colab-include-cycle-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.codemod");
        let b = dir.join("b.codemod");
        std::fs::write(&a, r#"refactor "a" { include "b.codemod" }"#).unwrap();
        std::fs::write(&b, r#"refactor "b" { include "a.codemod" }"#).unwrap();

        let err = compile_at_path(&a, &registry()).unwrap_err();
        assert!(
            matches!(&err, Error::Config(msg) if msg.contains("circular include")),
            "got: {err}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
