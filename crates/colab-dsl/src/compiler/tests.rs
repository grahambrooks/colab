use super::*;
use crate::ast::{Action, Match, Namespace};
use colab_core::CodeTransformer;

fn registry() -> BackendRegistry {
    // The real registry, so a compiler test cannot pass against a
    // backend set the binary does not actually ship.
    colab_backends::registry()
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
fn parses_an_in_scope_clause() {
    let result = grammar::MatchParser::new()
        .parse(r#"match go::symbol "A" in "internal/**" { replace "B" }"#)
        .unwrap();
    assert_eq!(result.scope.as_deref(), Some("internal/**"));
}

#[test]
fn a_scoped_rule_only_applies_inside_the_glob() {
    let refactoring = compile(
        r#"refactor "s" { match go::symbol "Old" in "internal/**" { replace "New" } }"#,
        &registry(),
    )
    .unwrap();

    let source = "package demo\n\nfunc Old() {}\n";
    assert!(refactoring.is_file_relevant(Path::new("internal/a.go")));
    assert!(!refactoring.is_file_relevant(Path::new("cmd/a.go")));

    let inside = refactoring.apply_at(Path::new("internal/a.go"), source);
    assert!(
        inside.output.contains("func New()"),
        "got: {}",
        inside.output
    );
    assert_eq!(inside.rules_fired, vec![0]);

    let outside = refactoring.apply_at(Path::new("cmd/a.go"), source);
    assert_eq!(outside.output, source);
    assert!(outside.rules_fired.is_empty());
}

#[test]
fn an_unscoped_rule_is_unaffected() {
    let refactoring = compile(
        r#"refactor "s" { match go::symbol "Old" { replace "New" } }"#,
        &registry(),
    )
    .unwrap();
    assert!(refactoring.is_file_relevant(Path::new("cmd/a.go")));
}

#[test]
fn the_scope_appears_in_the_rule_display() {
    let refactoring = compile(
        r#"refactor "s" { match go::symbol "Old" in "internal/**" { replace "New" } }"#,
        &registry(),
    )
    .unwrap();
    assert_eq!(
        refactoring.rules[0].to_string(),
        "go::symbol \"Old\" -> \"New\" in \"internal/**\""
    );
}

#[test]
fn an_invalid_scope_glob_fails_at_compile_time() {
    let err = compile(
        r#"refactor "s" { match go::symbol "Old" in "internal/[" { replace "New" } }"#,
        &registry(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("invalid scope glob"), "{err}");
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
            scope: None,
            fences: None,
            prose: false,
            message: None,
            severity: None,
            id: None,
            url: None,
            constraints: Vec::new(),
            lets: Vec::new(),
            action: Action::Replace("d.e.f".to_string()),
            imports: Vec::new(),
            span: crate::ast::Span { start: 1, end: 46 },
        }
    );
}

#[test]
fn parses_full_program_with_single_match() {
    let command =
        parse(r#" refactor "this" { match  go::import "a.b.c" { replace "d.e.f" } } "#).unwrap();

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
    let command = syntax::parse(
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
    let command = syntax::parse(
        r#"
            refactor "with-include" {
                include "shared.colab"
                match go::import "x" { replace "y" }
            }
            "#,
    )
    .unwrap();
    assert_eq!(command.items.len(), 2);
    assert!(matches!(command.items[0], Item::Include(ref s) if s == "shared.colab"));
    assert!(matches!(command.items[1], Item::Match(_)));
}

#[test]
fn ignores_line_comments() {
    let command = syntax::parse(
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
    let command = syntax::parse(r#"refactor "noop" { }"#).unwrap();
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
    let err = compile(r#"refactor "x" { include "other.colab" }"#, &registry()).unwrap_err();
    assert!(
        matches!(&err, Error::Config(msg) if msg.contains("compile_at_path")),
        "got: {err}"
    );
}

#[test]
fn a_parse_error_names_the_file_it_is_in() {
    let dir = std::env::temp_dir().join(format!("colab-parse-path-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let main = dir.join("main.colab");
    let shared = dir.join("shared.colab");
    std::fs::write(&main, "include \"shared.colab\"\n").unwrap();
    std::fs::write(&shared, "match go::import \"a\" { nope }\n").unwrap();
    let backends = colab_backends::registry();
    let Err(Error::Parse(detail)) = compile_at_path(&main, &backends) else {
        panic!("a parse error");
    };
    assert_eq!(detail.path.as_deref(), Some(shared.as_path()));
    assert_eq!((detail.line, detail.column), (1, 24));
    let message = Error::Parse(detail).to_string();
    assert!(message.starts_with("parse error in "), "{message}");
    assert!(
        message.contains("shared.colab at line 1, column 24"),
        "{message}"
    );
    // Text that is not a file (`-e`, stdin, MCP) names none.
    let Err(Error::Parse(inline)) = compile_at_source(
        "match go::import \"a\" { nope }",
        &dir.join("<inline>"),
        &backends,
    ) else {
        panic!("a parse error");
    };
    assert_eq!(inline.path, None);
    std::fs::remove_dir_all(&dir).unwrap();
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
    let main = dir.join("main.colab");
    let shared = dir.join("shared.colab");
    std::fs::write(
        &main,
        r#"refactor "outer" { include "shared.colab" match go::import "z" { replace "Z" } }"#,
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
    let a = dir.join("a.colab");
    let b = dir.join("b.colab");
    std::fs::write(&a, r#"refactor "a" { include "b.colab" }"#).unwrap();
    std::fs::write(&b, r#"refactor "b" { include "a.colab" }"#).unwrap();

    let err = compile_at_path(&a, &registry()).unwrap_err();
    assert!(
        matches!(&err, Error::Config(msg) if msg.contains("circular include")),
        "got: {err}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn check_reports_every_bad_rule_at_its_own_span() {
    let backends = colab_backends::registry();
    let text = "refactor \"x\" {\n  match go::import \"a\" { replace \"b\" }\n  match klingon::x \"a\" { delete }\n  match go::pattern 'func (' { delete }\n}\n";
    let checks = check(text, &backends).unwrap();
    assert_eq!(checks.len(), 3);
    assert!(checks[0].outcome.is_ok());
    assert!(checks[1].outcome.is_err());
    assert!(checks[2].outcome.is_err());
    let second = &text[checks[1].span.start..checks[1].span.end];
    assert_eq!(second, "match klingon::x \"a\" { delete }");
    assert_eq!(checks[2].namespace, "go::pattern");
    assert_eq!(checks[2].action, "delete");
}

// ---------- where / import clauses (M26) ------------------------------

fn one_match(rule: &str) -> Match {
    let command = syntax::parse(&format!("refactor \"t\" {{ {rule} }}")).unwrap();
    match command.items.into_iter().next() {
        Some(Item::Match(m)) => m,
        other => panic!("expected a match, got {other:?}"),
    }
}

#[test]
fn fence_clauses_parse_once_and_combine_with_a_glob() {
    let m = one_match(r#"match go::import "a" in "docs/**" and fences { replace "b" }"#);
    assert_eq!(m.scope.as_deref(), Some("docs/**"));
    assert_eq!(m.fences, Some(crate::ast::Fences::Also));
    let m = one_match(r#"match go::import "a" in fences { replace "b" }"#);
    assert_eq!((m.scope, m.fences), (None, Some(crate::ast::Fences::Only)));
    let twice =
        crate::parse(r#"refactor "r" { match go::import "a" in fences and fences { delete } }"#);
    assert!(twice.unwrap_err().to_string().contains("once"));
    // `fences` and `and` still spell namespaces.
    assert!(crate::parse(r#"refactor "r" { match x::fences "a" { delete } }"#).is_ok());
}

#[test]
fn message_and_severity_clauses_reach_the_report() {
    let backends = colab_backends::registry();
    let r = compile(
        r#"refactor "r" { match go::import "a" { delete requires import "b" message "a is gone" severity warning } match go::import "c" { delete } }"#,
        &backends,
    )
    .unwrap();
    let report = r.report();
    assert_eq!(report.rules()[0].message.as_deref(), Some("a is gone"));
    assert_eq!(report.rules()[0].severity, colab_core::Severity::Warning);
    assert_eq!(report.rules()[1].severity, colab_core::Severity::Error);
    for bad in [
        r#"refactor "r" { match go::import "a" { delete severity loud } }"#,
        r#"refactor "r" { match go::import "a" { delete message "x" message "y" } }"#,
    ] {
        assert!(crate::parse(bad).is_err(), "{bad}");
    }
    assert!(crate::parse(r#"refactor "r" { match x::message "a" { delete } }"#).is_ok());
}

#[test]
fn and_prose_renames_in_comments_and_strings_too() {
    let backends = colab_backends::registry();
    let r = compile(
        r#"refactor "r" { match go::symbol "ParseConfig" and prose { replace "LoadConfig" } }"#,
        &backends,
    )
    .unwrap();
    let src = "package c\n\n// ParseConfig reads; see ParseConfigs.\nfunc ParseConfig() { log(\"ParseConfig\") }\n";
    assert_eq!(
        r.rules[0].apply(src),
        "package c\n\n// LoadConfig reads; see ParseConfigs.\nfunc LoadConfig() { log(\"LoadConfig\") }\n"
    );
    assert_eq!(
        r.rules[0].to_string(),
        r#"go::symbol "ParseConfig" -> "LoadConfig" and prose"#
    );
    let wrong = compile(
        r#"refactor "r" { match go::import "a" and prose { replace "b" } }"#,
        &backends,
    );
    assert!(
        wrong
            .unwrap_err()
            .to_string()
            .contains("goes with a `symbol` rename")
    );
}

#[test]
fn a_rewrite_that_matches_its_own_output_is_advised_once() {
    let backends = colab_backends::registry();
    let r = compile(
        r#"refactor "r" { match go::call "f" and fences { replace_call "f(ctx, $args)" } match go::call "f" { replace_call "g($args)" } }"#,
        &backends,
    )
    .unwrap();
    let report = r.report();
    let warnings = report.warnings();
    // Two rules from `and fences`, one warning; the rename needs none.
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("still calls `f`"), "{warnings:?}");
}

#[test]
fn and_fences_lowers_to_the_rule_and_its_fenced_twin() {
    let backends = colab_backends::registry();
    let both = compile(
        r#"refactor "r" { match go::import "a/b" and fences { replace "c/d" } }"#,
        &backends,
    )
    .unwrap();
    let shown: Vec<String> = both
        .rules
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    assert_eq!(
        shown,
        [
            r#"go::import "a/b" -> "c/d""#,
            r#"go::import "a/b" -> "c/d" in fences"#
        ]
    );
    let only = compile(
        r#"refactor "r" { match go::import "a/b" in "docs/**" in fences { delete } }"#,
        &backends,
    )
    .unwrap();
    assert_eq!(only.rules.len(), 1);
    assert!(only.rules[0].is_file_relevant(Path::new("docs/x.md")));
    assert!(!only.rules[0].is_file_relevant(Path::new("README.md")));
    // `markdown::text` reads fences itself: one rule, no fenced twin.
    let text = compile(
        r#"refactor "r" { match markdown::text "A[x y]" and fences in "docs/**" { replace "z" } }"#,
        &backends,
    )
    .unwrap();
    assert_eq!(text.rules.len(), 1);
    assert!(
        text.rules[0].to_string().contains("and fences"),
        "{}",
        text.rules[0]
    );
    assert!(
        !text.rules[0].is_file_relevant(Path::new("README.md")),
        "scope kept"
    );
    let none = compile(
        r#"refactor "r" { match markdown::section "A" in fences { delete } }"#,
        &backends,
    );
    assert!(
        none.unwrap_err()
            .to_string()
            .contains("cannot run in Markdown code fences")
    );
}

#[test]
fn an_escaped_quote_says_the_dsl_has_no_escapes() {
    let script = "refactor \"r\" { match markdown::section \"a \\\"b\\\" c\" { delete } }";
    let err = crate::parse(script).unwrap_err().to_string();
    assert!(err.contains("DSL strings have no escapes"), "{err}");
}

#[test]
fn a_raw_string_closed_early_says_where() {
    let script =
        "refactor \"r\" {\n  match toml::key \"a\" { set r#\"see \"#anchor\" here\"# }\n}\n";
    let err = crate::parse(script).unwrap_err().to_string();
    assert!(
        err.contains("closed at the `\"#` on line 2, column 36"),
        "{err}"
    );
    assert!(err.contains("r##"), "{err}");
}

#[test]
fn raw_strings_hold_both_quotes_and_span_lines() {
    let script = "refactor \"r\" {\n  match toml::key r#\"a\"# { set r#\"say \"hi\" isn't\n'\"x\"'\"# }\n}\n";
    let parsed = crate::parse(script).unwrap();
    let crate::ast::Item::Match(m) = &parsed.items[0] else {
        panic!("a match")
    };
    assert_eq!(m.match_string, "a");
    assert_eq!(
        m.action,
        crate::ast::Action::Set("say \"hi\" isn't\n'\"x\"'".into())
    );
    let two =
        "refactor \"r\" { match toml::key \"a\" { set r##\"a raw string ends at \"# here\"## } }";
    let parsed = crate::parse(two).unwrap();
    let crate::ast::Item::Match(m) = &parsed.items[0] else {
        panic!("a match")
    };
    assert_eq!(
        m.action,
        crate::ast::Action::Set("a raw string ends at \"# here".into())
    );
}

#[test]
fn parses_every_constraint_form() {
    let m = one_match(
        r#"match go::pattern 'f($A, $$$R)' where $A matches /^"a\/b/, $A is identifier, not $A matches "x", $$$R count 1..3, $$$R count 2.., $$$R count ..4, $$$R count 0, inside 'func $F() { $$$ }', not has 'g()' { delete }"#,
    );
    let shown: Vec<String> = m
        .constraints
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    assert_eq!(
        shown,
        [
            r#"$A matches /^"a\/b/"#,
            "$A is identifier",
            "not $A matches /x/",
            "$$$R count 1..3",
            "$$$R count 2..",
            "$$$R count 0..4",
            "$$$R count 0",
            "inside 'func $F() { $$$ }'",
            "not has 'g()'",
        ]
    );
}

#[test]
fn parses_import_clauses_after_the_action() {
    let m = one_match(
        r#"match go::pattern 'f($A)' { replace 'g($A)' requires import "g" drops import "f" when unused }"#,
    );
    assert_eq!(
        m.imports,
        [
            ImportClause::Requires("g".into()),
            ImportClause::DropsUnused("f".into())
        ]
    );
    assert!(m.constraints.is_empty());
}

#[test]
fn new_keywords_still_spell_namespaces() {
    // `import` became a keyword for `requires import`; `go::import`
    // must still parse.
    let m = one_match(r#"match go::import "a" { replace "b" }"#);
    assert_eq!(m.namespace.module, "import");
}

#[test]
fn a_where_clause_reaches_the_pattern_and_its_display() {
    let script = r#"refactor "t" { match go::pattern 'errors.Wrap($E, $M)' where $M matches /^"/ { replace 'fmt.Errorf($M, $E)' requires import "fmt" } }"#;
    let refactoring = compile(script, &registry()).unwrap();
    assert_eq!(
        refactoring.rules[0].to_string(),
        r#"go::pattern 'errors.Wrap($E, $M)' where $M matches /^"/ -> 'fmt.Errorf($M, $E)' requires import "fmt""#
    );
    let src = "package p\n\nfunc f() {\n\terrors.Wrap(err, \"a\")\n\terrors.Wrap(err, m)\n}\n";
    let out = refactoring.rules[0].apply(src);
    assert_eq!(
        out,
        "package p\n\nimport \"fmt\"\n\nfunc f() {\n\tfmt.Errorf(\"a\", err)\n\terrors.Wrap(err, m)\n}\n"
    );
}

#[test]
fn parses_relations_positions_and_groups() {
    let m = one_match(
        r#"match go::pattern 'defer $X.Close()' where follows 'if $C { $$$ }' directly, precedes 'return nil', inside 'for $$$ { $$$ }' until 'func() { $$$ }', has 'f()' directly, not any(nth 1, nth -1, all(field body, $X is identifier)) { delete }"#,
    );
    let shown: Vec<String> = m
        .constraints
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    assert_eq!(
        shown,
        [
            "follows 'if $C { $$$ }' directly",
            "precedes 'return nil'",
            "inside 'for $$$ { $$$ }' until 'func() { $$$ }'",
            "has 'f()' directly",
            "not any(nth 1, nth -1, all(field body, $X is identifier))",
        ]
    );
    // The keywords stay usable where an identifier goes.
    let m = one_match(r#"match go::pattern '$X' where $X is field { delete }"#);
    assert_eq!(m.constraints[0].to_string(), "$X is field");
}

#[test]
fn scans_and_stages_compile_and_in_names_a_scan_before_it() {
    let script = r#"refactor "t" {
            match go::import "a" { replace "b" }
            then {
                scan go::pattern 'func $F($$$P) error { $$$B }' in "api/**" as fails
                match go::pattern 'api.$F($$$A)' where $F in fails { replace 'api.$F(ctx, $$$A)' }
            }
        }"#;
    let r = compile(script, &registry()).unwrap();
    assert!(r.has_scans());
    let starts: Vec<usize> = r.stages.iter().map(|s| s.start).collect();
    // The stage the `then` opens, at rule 1 (an empty one may follow).
    assert_eq!(starts[0], 1);
    assert_eq!(r.stages[0].scans[0].name, "fails");
    assert!(
        r.rules[1].to_string().contains("$F in fails.$F"),
        "{}",
        r.rules[1]
    );
    let err = |script: &str| compile(script, &registry()).err().unwrap().to_string();
    // Used before the scan exists.
    let e = err(
        r#"refactor "t" { match go::pattern 'f($X)' where $X in later { delete } then { scan go::pattern 'g($Y)' as later } }"#,
    );
    assert!(e.contains("no scan named `later` before this rule"), "{e}");
    let e = err(
        r#"refactor "t" { scan go::pattern 'f($A, $B)' as two match go::pattern 'g($X)' where $X in two { delete } }"#,
    );
    assert!(e.contains("binds `$A`, `$B`; say which"), "{e}");
    let e = err(
        r#"refactor "t" { scan go::pattern 'f($A)' as one match go::pattern 'g($X)' where $X in one.$Q { delete } }"#,
    );
    assert!(e.contains("binds `$A`"), "{e}");
    let e = err(r#"refactor "t" { scan go::import "x" as imports }"#);
    assert!(e.contains("is `go::pattern '<code>'`"), "{e}");
    let e = err(r#"refactor "t" { scan go::pattern 'f($A)' as s scan go::pattern 'g($A)' as s }"#);
    assert!(e.contains("two scans are named `s`"), "{e}");
    // Without scans, stages are only rule order: no phased run.
    let plain = compile(r#"refactor "t" { match go::import "a" { replace "b" } then { match go::import "b" { replace "c" } } }"#, &registry()).unwrap();
    assert!(!plain.has_scans());
    assert_eq!(
        plain.apply("package p\n\nimport \"a\"\n"),
        "package p\n\nimport \"c\"\n"
    );
}

#[test]
fn named_patterns_resolve_wherever_a_constraint_takes_one() {
    let script = r#"refactor "t" {
            match go::pattern 'use($X)' where inside loop until closure, not any(follows loop directly) { delete }
            pattern loop = 'for $$$H { $$$B }'
            pattern closure = 'func() { $$$B }'
        }"#;
    let parsed = syntax::parse(script).unwrap();
    let crate::ast::Item::Match(m) = &parsed.items[0] else {
        panic!("a match first");
    };
    // Unresolved, a reference shows as the bare name.
    let shown: Vec<String> = m.constraints.iter().map(ToString::to_string).collect();
    assert_eq!(
        shown,
        [
            "inside loop until closure",
            "not any(follows loop directly)"
        ]
    );
    // Defined after its use, and resolved to the text.
    let refactoring = compile(script, &registry()).unwrap();
    assert_eq!(
        refactoring.rules[0].to_string(),
        "go::pattern 'use($X)' where inside 'for $$$H { $$$B }' until 'func() { $$$B }', not any(follows 'for $$$H { $$$B }' directly) -> delete"
    );
    let src = "package p\nfunc f() {\n\tfor _, x := range xs {\n\t\tuse(x)\n\t\tgo func() { use(x) }()\n\t}\n}\n";
    assert_eq!(
        refactoring.rules[0].apply(src),
        "package p\nfunc f() {\n\tfor _, x := range xs {\n\t\tgo func() { use(x) }()\n\t}\n}\n"
    );
    // `check` resolves the file's own names too.
    let checks = check(script, &registry()).unwrap();
    assert!(checks[0].outcome.is_ok(), "{:?}", checks[0].outcome);
}

#[test]
fn a_rewriter_is_named_once_and_used_by_a_let() {
    let script = r#"refactor "t" {
            rewriter wrap = 'g($X)' -> 'h($X)'
            match go::pattern 'f($$$A)' { let $N = $$$A | rewrite(wrap) replace 'ff($N)' }
        }"#;
    let parsed = syntax::parse(script).unwrap();
    let crate::ast::Item::Match(m) = &parsed.items[1] else {
        panic!("a match second");
    };
    assert_eq!(m.lets[0].to_string(), "let $N = $$$A | rewrite(wrap)");
    let refactoring = compile(script, &registry()).unwrap();
    assert_eq!(
        refactoring.rules[0].to_string(),
        "go::pattern 'f($$$A)' let $N = $$$A | rewrite(wrap) -> 'ff($N)'"
    );
    assert_eq!(
        refactoring.rules[0].apply("package p\n\nfunc m() { f(g(1), 2) }\n"),
        "package p\n\nfunc m() { ff(h(1), 2) }\n"
    );
    let err = |script: &str| compile(script, &registry()).err().unwrap().to_string();
    let e = err(
        r#"refactor "t" { rewriter wrap = 'g($X)' -> 'h($X)' match go::pattern 'f($$$A)' { let $N = $$$A | rewrite(wrp) replace 'ff($N)' } }"#,
    );
    assert!(
        e.contains("no rewriter named `wrp` (did you mean `wrap`?)"),
        "{e}"
    );
    let e = err(r#"refactor "t" { rewriter a = 'x' -> 'y' rewriter a = 'x' -> 'z' }"#);
    assert!(e.contains("`rewriter a` is defined twice"), "{e}");
    let e = err(
        r#"refactor "t" { match go::pattern 'f($$$A)' { let $N = $$$A | rewrite("wrap") replace 'ff($N)' } }"#,
    );
    assert!(e.contains("takes the names of rewriters"), "{e}");
}

#[test]
fn type_and_resolves_parse_and_mark_the_rule_as_needing_a_server() {
    let m = one_match(
        r#"match go::pattern '$F($$$A)' where $F resolves "github.com/pkg/errors.Wrap", any($$$A type "error", nth 1) { delete }"#,
    );
    let shown: Vec<String> = m.constraints.iter().map(ToString::to_string).collect();
    assert_eq!(
        shown,
        [
            r#"$F resolves "github.com/pkg/errors.Wrap""#,
            r#"any($$$A type "error", nth 1)"#
        ]
    );
    let plain = compile(
        r#"refactor "t" { match go::pattern 'f($A)' { delete } }"#,
        &registry(),
    )
    .unwrap();
    assert!(!plain.needs_oracle());
    let asks = compile(
        r#"refactor "t" { match go::pattern 'f($A)' where any(nth 1, $A type "int") { delete } }"#,
        &registry(),
    )
    .unwrap();
    assert!(asks.needs_oracle());
    let e = compile(
        r#"refactor "t" { match go::pattern 'f($A)' where $B type "int" { delete } }"#,
        &registry(),
    )
    .err()
    .unwrap()
    .to_string();
    assert!(e.contains("`$B` is not a metavariable"), "{e}");
}

#[test]
fn a_rule_takes_one_id_and_url_and_ids_are_unique() {
    let m = one_match(
        r#"match go::import "a" { replace "b" id "no-a" url "https://example.test" message "m" }"#,
    );
    assert_eq!(
        (m.id.as_deref(), m.url.as_deref()),
        (Some("no-a"), Some("https://example.test"))
    );
    let err = |script: &str| compile(script, &registry()).err().unwrap().to_string();
    assert!(
        err(r#"refactor "t" { match go::import "a" { replace "b" id "x" id "y" } }"#)
            .contains("one `id`")
    );
    assert!(
        err(r#"refactor "t" { match go::import "a" { replace "b" id "no a" } }"#)
            .contains("letters, digits")
    );
    let e = err(
        r#"refactor "t" { match go::import "a" { replace "b" id "x" } match go::import "c" { replace "d" id "x" } }"#,
    );
    assert!(e.contains(r#"two rules have `id "x"`"#), "{e}");
}

#[test]
fn an_unknown_or_repeated_pattern_name_fails_the_script() {
    let err = |script: &str| compile(script, &registry()).err().unwrap().to_string();
    let e = err(
        r#"refactor "t" { pattern loop = 'for { $$$ }' match go::pattern 'f()' where inside lop { delete } }"#,
    );
    assert!(
        e.contains("no pattern named `lop` (did you mean `loop`?)"),
        "{e}"
    );
    let e = err(r#"refactor "t" { match go::pattern 'f()' where has x { delete } }"#);
    assert!(e.contains("defines none"), "{e}");
    let e = err(r#"refactor "t" { pattern a = 'x()' pattern a = 'y()' }"#);
    assert!(e.contains("defined twice"), "{e}");
    // The per-rule check places a bad name on its rule.
    let checks = check(
        r#"refactor "t" { match go::pattern 'f()' where inside nope { delete } }"#,
        &registry(),
    )
    .unwrap();
    assert!(
        checks[0]
            .outcome
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("nope")
    );
    // With an include, it may be defined there: the rule is checked.
    let checks = check(
        r#"refactor "t" { include "shared.colab" match go::pattern 'f()' where inside shared { delete } }"#,
        &registry(),
    )
    .unwrap();
    assert!(checks[0].outcome.is_ok(), "{:?}", checks[0].outcome);
}

#[test]
fn a_let_parses_every_step_form_and_reaches_the_template() {
    let m = one_match(
        r#"match go::pattern '$X.$F()' { let $N = $F | strip_prefix("Get") | replace(/^(a)/, "b") | substring(1, -1) | snake let $M = $N | upper replace '$X.$M()' }"#,
    );
    let shown: Vec<String> = m.lets.iter().map(ToString::to_string).collect();
    assert_eq!(
        shown,
        [
            r#"let $N = $F | strip_prefix("Get") | replace(/^(a)/, "b") | substring(1, -1) | snake"#,
            "let $M = $N | upper",
        ]
    );
    let script = r#"refactor "t" { match go::pattern '$X.$F()' where $F matches /^Get/ { let $N = $F | strip_prefix("Get") replace '$X.$N()' } }"#;
    let refactoring = compile(script, &registry()).unwrap();
    assert_eq!(
        refactoring.rules[0].to_string(),
        r#"go::pattern '$X.$F()' where $F matches /^Get/ let $N = $F | strip_prefix("Get") -> '$X.$N()'"#
    );
    let src = "package p\n\nfunc f() { u.GetName(); u.Name() }\n";
    assert_eq!(
        refactoring.rules[0].apply(src),
        "package p\n\nfunc f() { u.Name(); u.Name() }\n"
    );
}

#[test]
fn a_let_is_refused_where_it_cannot_apply() {
    let err = |rule: &str| {
        compile(&format!("refactor \"t\" {{ {rule} }}"), &registry())
            .err()
            .map_or_default(|e| e.to_string())
    };
    let e = err(r#"match go::symbol "a" { let $N = $F | lower replace "b" }"#);
    assert!(e.contains("only `go::pattern` binds metavariables"), "{e}");
    let e = err(r#"match go::pattern '$X.$F()' { let $N = $F | lower delete }"#);
    assert!(e.contains("has no template"), "{e}");
    let e = err(r#"match go::pattern '$X.$F()' { let $N = $F | snak replace '$N' }"#);
    assert!(e.contains("did you mean `snake`"), "{e}");
    let e = err(r#"match go::pattern '$X.$F()' { let $N = $F | strip_prefix(1) replace '$N' }"#);
    assert!(e.contains("takes one string"), "{e}");
    let e = err(r#"match go::pattern '$X.$F()' { let $N = $F | lower(1) replace '$N' }"#);
    assert!(e.contains("takes no arguments"), "{e}");
}

#[test]
fn where_and_import_clauses_are_rejected_where_they_cannot_apply() {
    let err = |rule: &str| {
        compile(&format!("refactor \"t\" {{ {rule} }}"), &registry())
            .err()
            .map_or_default(|e| e.to_string())
    };
    let e = err(r#"match go::import "a" where $A is identifier { replace "b" }"#);
    assert!(
        e.contains("`go::import` does not take a `where` clause"),
        "{e}"
    );
    let e = err(r#"match json::key "/a" where $A is identifier { set '1' }"#);
    assert!(e.contains("does not take a `where` clause"), "{e}");
    let e = err(r#"match go::pattern 'f($A)' where $B is identifier { delete }"#);
    assert!(e.contains("`$B` is not a metavariable"), "{e}");
    let e = err(r#"match json::key "/a" { set '1' requires import "x" }"#);
    assert!(e.contains("`json` has no import module"), "{e}");
    let e = err(r#"match python::pattern 'f($A)' { delete drops import "os" when unused }"#);
    assert!(
        e.contains("cannot tell whether an import is still used"),
        "{e}"
    );
}
