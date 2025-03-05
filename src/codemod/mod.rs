use lalrpop_util::lalrpop_mod;
use std::error::Error;

lalrpop_mod!(pub codemod, "/codemod/codemod.rs");

#[derive(PartialEq, Debug)]
pub struct Command {
    pub refactor_name: String,
    pub body: Body,
}

#[derive(PartialEq, Debug)]
pub struct Body {
    pub namespace: Namespace,
    pub match_string: String,
    pub action: Action,
}

#[derive(PartialEq, Debug)]
pub struct Namespace {
    pub lang: String,
    pub module: String,
}
#[derive(PartialEq, Debug)]
pub enum Action {
    Replace(String),
}

pub fn parse(text: &str) -> Result<Command, Box<dyn Error + '_>> {
    let result = codemod::ProgramParser::new().parse(text);
    match result {
        Ok(command) => Ok(command),
        Err(error) => Err(Box::new(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codemod::Action::Replace;

    #[test]
    fn test_parse_string_literal() {
        let result = codemod::StringLiteralParser::new()
            .parse(r#"  "Hello, World!"  "#)
            .unwrap();
        assert_eq!(result, "Hello, World!");
    }
    #[test]
    fn test_parse_identifier() {
        let result = codemod::IdentifierParser::new()
            .parse(r#"HelloWorld122 "#)
            .unwrap();
        assert_eq!(result, "HelloWorld122");
    }

    #[test]
    fn test_parse_action() {
        let result = codemod::ActionParser::new()
            .parse(r#"  replace "a.b.c" "#)
            .unwrap();
        assert_eq!(result, Replace("a.b.c".to_string()));
    }
    #[test]
    fn test_parse_namespace() {
        let result = codemod::NamespaceParser::new()
            .parse(r#"  go::import "#)
            .unwrap();
        assert_eq!(
            result,
            Namespace {
                lang: "go".to_string(),
                module: "import".to_string()
            }
        );
    }

    #[test]
    fn test_parse_body() {
        let result = codemod::BodyParser::new()
            .parse(r#" match  go::import "a.b.c" { replace "d.e.f" } "#)
            .unwrap();
        assert_eq!(
            result,
            Body {
                action: Action::Replace("d.e.f".to_string()),
                match_string: "a.b.c".to_string(),
                namespace: Namespace {
                    lang: "go".to_string(),
                    module: "import".to_string()
                }
            }
        );
    }
    #[test]
    fn test_parse_program() {
        let result =
            parse(r#" refactor "this" { match  go::import "a.b.c" { replace "d.e.f" } } "#)
                .unwrap();
        assert_eq!(
            result,
            Command {
                refactor_name: "this".to_string(),
                body: Body {
                    action: Action::Replace("d.e.f".to_string()),
                    match_string: "a.b.c".to_string(),
                    namespace: Namespace {
                        lang: "go".to_string(),
                        module: "import".to_string()
                    }
                }
            }
        );
    }
}
