//! Parse codemod scripts and lower them into the runtime IR.
//!
//! The grammar (see `codemod.lalrpop`) is consumed by [`parse`], which
//! returns a raw [`Command`] AST. [`compile`] then validates the AST and
//! produces an executable [`Refactoring`].

use lalrpop_util::lalrpop_mod;

use crate::codemod::ast::{Action, Command, Namespace};
use crate::codemod::model::{Refactoring, Rule};
use crate::error::{Error, Result};

lalrpop_mod!(grammar, "/codemod/codemod.rs");

/// Parse `text` into a raw AST without applying any semantic checks.
pub fn parse(text: &str) -> Result<Command> {
    grammar::ProgramParser::new()
        .parse(text)
        .map_err(|e| Error::Parse(e.to_string()))
}

/// Parse and validate `text`, returning an executable [`Refactoring`].
///
/// Returns [`Error::UnsupportedOperation`] when the script targets a
/// namespace or action that colab does not implement.
pub fn compile(text: &str) -> Result<Refactoring> {
    let command = parse(text)?;
    let rule = lower_rule(command.body.namespace, command.body.match_string, command.body.action)?;
    Ok(Refactoring {
        name: command.refactor_name,
        rule,
    })
}

fn lower_rule(namespace: Namespace, target: String, action: Action) -> Result<Rule> {
    match (namespace.lang.as_str(), namespace.module.as_str(), action) {
        ("go", "import", Action::Replace(replacement)) => Ok(Rule::GoImportRename {
            from: target,
            to: replacement,
        }),
        (lang, module, _) => Err(Error::UnsupportedOperation(format!(
            "{}::{} is not a supported namespace",
            lang, module
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codemod::ast::{Action, Body, Namespace};

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
    fn parses_body() {
        let result = grammar::BodyParser::new()
            .parse(r#" match  go::import "a.b.c" { replace "d.e.f" } "#)
            .unwrap();
        assert_eq!(
            result,
            Body {
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
    fn parses_full_program() {
        let command = parse(
            r#" refactor "this" { match  go::import "a.b.c" { replace "d.e.f" } } "#,
        )
        .unwrap();

        assert_eq!(command.refactor_name, "this");
        assert_eq!(command.body.namespace.lang, "go");
        assert_eq!(command.body.namespace.module, "import");
        assert_eq!(command.body.match_string, "a.b.c");
        assert_eq!(command.body.action, Action::Replace("d.e.f".to_string()));
    }

    #[test]
    fn compiles_go_import_replace() {
        let refactoring =
            compile(r#"refactor "rename" { match go::import "old" { replace "new" } }"#).unwrap();
        assert_eq!(refactoring.name, "rename");
        match refactoring.rule {
            Rule::GoImportRename { from, to } => {
                assert_eq!(from, "old");
                assert_eq!(to, "new");
            }
        }
    }

    #[test]
    fn rejects_unsupported_namespace() {
        let err = compile(r#"refactor "x" { match rust::module "a" { replace "b" } }"#).unwrap_err();
        assert!(matches!(err, Error::UnsupportedOperation(_)));
    }
}
