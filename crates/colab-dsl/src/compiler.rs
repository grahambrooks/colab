//! Parse codemod scripts and lower them into the runtime IR.
//!
//! The grammar (see `codemod.lalrpop`) is consumed by [`parse`], which
//! returns a raw [`Command`] AST. [`compile`] then validates the AST
//! and asks the supplied [`BackendRegistry`] to produce one
//! [`Operation`] per match block, yielding an executable
//! [`Refactoring`].

use lalrpop_util::lalrpop_mod;

use crate::ast::{Action, Command, Match};
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
pub fn compile(text: &str, backends: &BackendRegistry) -> Result<Refactoring> {
    let command = parse(text)?;
    let mut rules: Vec<Box<dyn Operation>> = Vec::with_capacity(command.matches.len());
    for m in command.matches {
        rules.push(lower_match(m, backends)?);
    }
    Ok(Refactoring {
        name: command.refactor_name,
        rules,
    })
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
        let result = grammar::NamespaceParser::new().parse("  go::import ").unwrap();
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
        let command = parse(
            r#" refactor "this" { match  go::import "a.b.c" { replace "d.e.f" } } "#,
        )
        .unwrap();

        assert_eq!(command.refactor_name, "this");
        assert_eq!(command.matches.len(), 1);
        let m = &command.matches[0];
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

        assert_eq!(command.matches.len(), 3);
        assert_eq!(command.matches[0].match_string, "a");
        assert_eq!(command.matches[1].match_string, "c");
        assert_eq!(command.matches[2].match_string, "e");
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
        assert_eq!(command.matches.len(), 2);
    }

    #[test]
    fn parses_empty_refactor_block() {
        let command = parse(r#"refactor "noop" { }"#).unwrap();
        assert_eq!(command.refactor_name, "noop");
        assert!(command.matches.is_empty());
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
        assert!(display.contains("go::import \"old\" -> \"new\""), "got: {display}");
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
        // First rule turns old → mid, second turns mid → new.
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
}
