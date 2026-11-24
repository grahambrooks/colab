//! The pattern engine's unit tests: compiling, matching, constraints,
//! rewriting and deleting, across a few grammars.

use super::*;

fn go() -> PatternLanguage {
    PatternLanguage {
        grammar: tree_sitter_go::LANGUAGE.into(),
        expando: 'µ',
        contexts: &["package p\nfunc _() {\n@@PATTERN@@\n}\n"],
        also: &[],
        qualified_calls: None,
        leading: Leading::NONE,
        macro_contents: None,
        modifiers: None,
        shorthand_fields: false,
        modifier_tokens: &[],
    }
}

fn python() -> PatternLanguage {
    PatternLanguage {
        grammar: tree_sitter_python::LANGUAGE.into(),
        expando: 'µ',
        contexts: &[],
        also: &["def _(x: @@PATTERN@@): pass\n"],
        qualified_calls: None,
        leading: Leading::NONE,
        macro_contents: None,
        modifiers: None,
        shorthand_fields: false,
        modifier_tokens: &[],
    }
}

const GO_SRC: &str = "package main\n\nfunc main() {\n\tif err := f(); err != nil {\n\t\treturn errors.Wrap(err, \"f failed\")\n\t}\n\tlog.Println(\"a\", 1)\n}\n";

#[test]
fn an_empty_sequence_takes_its_separator_with_it() {
    assert_eq!(
        tighten("{ log(); $B }", &["B"]).as_deref(),
        Some("{ log(); }")
    );
    assert_eq!(tighten("g($R, 1)", &["R"]).as_deref(), Some("g(1)"));
    assert_eq!(tighten("g(1, $R)", &["R"]).as_deref(), Some("g(1)"));
    // Another name that starts the same is left alone.
    assert_eq!(tighten("f($BODY, $B)", &["B"]).as_deref(), Some("f($BODY)"));
    assert_eq!(tighten("f($A)", &["B"]), None);
    // End to end: the Go block gets no double space.
    let op = StructuralPattern::compile(&go(), "func $F() { $$$B }").unwrap();
    let out = op.rewrite("package p\n\nfunc a() {}\n", "func $F() { log(); $$$B }");
    assert_eq!(out, "package p\n\nfunc a() { log(); }\n");
}

#[test]
fn a_rewrite_keeps_the_layout_of_what_it_does_not_change() {
    // A one-line template over a many-line match changes one name.
    assert_eq!(
        keep_layout("f() {\n\tx := 1\n\treturn x\n}", "g() { x := 1 return x }"),
        "g() {\n\tx := 1\n\treturn x\n}"
    );
    // Next to a change, the template's spacing: tokens never fuse.
    assert_eq!(
        keep_layout("extern \"C\" {}", "unsafe extern \"C\" {}"),
        "unsafe extern \"C\" {}"
    );
    assert_eq!(keep_layout("x+y", "x in y"), "x in y");
    // Where the template breaks a line, its indentation stands.
    assert_eq!(
        keep_layout("if c:\n    a()", "if c:\n    with l:\n        a()"),
        "if c:\n    with l:\n        a()"
    );
    // After a deletion, the template's spacing; a closing brace on its
    // own line stays there.
    assert_eq!(keep_layout("w(err, \"m\")", "f(\"m\")"), "f(\"m\")");
    assert_eq!(
        keep_layout("if e {\n\treturn e\n}", "if e { return g(e) }"),
        "if e {\n\treturn g(e)\n}"
    );
    // A many-line capture in a one-line template keeps its lines'
    // indentation, and unwrapping a block keeps it unwrapped.
    assert_eq!(
        keep_layout("c({\n  t: 1,\n  u: 2,\n})", "d({ t: 1,\nu: 2, })"),
        "d({\n  t: 1,\n  u: 2,\n})"
    );
    assert_eq!(
        keep_layout("with l:\n    a()\n    b()", "a()\nb()"),
        "a()\nb()"
    );
    // A replaced token takes the place of the one it replaces.
    assert_eq!(
        keep_layout("c({\n  t: 1,\n  u: 2,\n})", "d({ tMs: 1 * 9,\nu: 2, })"),
        "d({\n  tMs: 1 * 9,\n  u: 2,\n})"
    );
    // A string is one token: its spaces are never taken from the old.
    assert_eq!(keep_layout("f(\"a  b\")", "f(\"a b\")"), "f(\"a b\")");
    // An element added before a list laid out one per line gets its
    // own line; added to a one-line list, it stays on that line.
    assert_eq!(
        keep_layout("build(\n    a,\n    b,\n)", "make(extra, a,\n    b,\n)"),
        "make(\n    extra,\n    a,\n    b,\n)"
    );
    assert_eq!(
        keep_layout("build(a, b)", "make(extra, a, b)"),
        "make(extra, a, b)"
    );
    // A token the template joins to the one before stays joined, though
    // it replaces one the original spaced: no `a:: b` from `a: b`.
    assert_eq!(
        keep_layout(
            "Capability {\n    module: \"pattern\",\n    description: $D,\n}",
            "colab_rewrite::language::pattern_capability($D)"
        ),
        "colab_rewrite::language::pattern_capability($D)"
    );
    // What the template appends to a sequence laid out one per line
    // starts a line of its own, indented like the line before: a
    // field after `$$$F`, an item after `$$$B`.
    assert_eq!(
        keep_layout(
            "PatternLanguage {\n        grammar: g,\n        also: &[],\n    }",
            "PatternLanguage { grammar: g,\n        also: &[], leading: NONE, }"
        ),
        "PatternLanguage {\n        grammar: g,\n        also: &[],\n        leading: NONE,\n    }"
    );
    assert_eq!(
        keep_layout(
            "impl L for R {\n    fn two(&self) -> u8 {\n        2\n    }\n}",
            "impl L for R { fn two(&self) -> u8 {\n        2\n    } fn three(&self) -> u8 { 3 } }"
        ),
        "impl L for R {\n    fn two(&self) -> u8 {\n        2\n    }\n    fn three(&self) -> u8 { 3 }\n}"
    );
    // On one line, appended text stays on it.
    assert_eq!(
        keep_layout("S { a: 1 }", "S { a: 1, b: 2 }"),
        "S { a: 1, b: 2 }"
    );
}

// ---------- relations, positions, any / all ---------------------------

const REL_SRC: &str = "package p\nfunc f() error {\n\tf, err := os.Open(\"x\")\n\tif err != nil {\n\t\treturn err\n\t}\n\tdefer f.Close()\n\tg(1, 2, 3)\n\tgo func() {\n\t\tfor _, x := range xs {\n\t\t\tuse(x)\n\t\t}\n\t}()\n\tfor _, y := range ys {\n\t\tgo func() { use(y) }()\n\t}\n\treturn nil\n}\n";

fn texts(pattern: &str, constraints: &[Constraint]) -> Vec<String> {
    constrained(pattern, constraints)
        .unwrap()
        .find(REL_SRC)
        .into_iter()
        .map(|f| f.text)
        .collect()
}

fn rel(relation: colab_core::Relation, pattern: &str, stop: colab_core::Stop) -> Test {
    Test::Related {
        relation,
        pattern: pattern.into(),
        stop,
    }
}

#[test]
fn sibling_relations_look_past_or_only_at_the_neighbour() {
    use colab_core::{Relation::*, Stop::*};
    let defer = "defer $X.Close()";
    let after_if = rel(Follows, "if err != nil { $$$ }", Directly);
    assert_eq!(texts(defer, &[c(false, after_if)]), ["defer f.Close()"]);
    let after_open = |stop| rel(Follows, "$A, err := $$$B", stop);
    assert!(texts(defer, &[c(false, after_open(Directly))]).is_empty());
    assert_eq!(texts(defer, &[c(false, after_open(Anywhere))]).len(), 1);
    // `until` stops at a sibling matching it.
    let past_if = after_open(Until("if $C { $$$ }".into()));
    assert!(texts(defer, &[c(false, past_if)]).is_empty());
    // A matched call is its statement: its siblings are statements.
    let before_return = rel(Precedes, "return nil", Anywhere);
    assert_eq!(texts("g($$$A)", &[c(false, before_return)]), ["g(1, 2, 3)"]);
}

#[test]
fn inside_and_has_take_a_stop() {
    use colab_core::{Relation::*, Stop::*};
    let use_ = "use($X)";
    let in_loop = |stop| c(false, rel(Inside, "for $$$ { $$$ }", stop));
    assert_eq!(texts(use_, &[in_loop(Anywhere)]).len(), 2);
    // The loop variable captured by a closure: `y`, not `x`.
    let closure = in_loop(Until("func() { $$$ }".into()));
    assert_eq!(texts(use_, &[closure]), ["use(x)"]);
    // `directly`: the nearest larger enclosing node only.
    let call = c(false, rel(Inside, "use($$$)", Directly));
    assert_eq!(
        texts(
            "$X",
            &[
                c(
                    false,
                    Test::Is {
                        var: var("$X"),
                        kind: "argument_list".into()
                    }
                ),
                call
            ]
        )
        .len(),
        2
    );
    let has_call = |stop| c(false, rel(Has, "use($$$)", stop));
    let funcs = "func() { $$$B }";
    assert_eq!(texts(funcs, &[has_call(Anywhere)]).len(), 2);
    assert!(texts(funcs, &[has_call(Until("for $$$ { $$$ }".into()))]).len() == 1);
    assert!(texts(funcs, &[has_call(Directly)]).is_empty());
}

#[test]
fn positions_fields_and_groups() {
    let int = || {
        c(
            false,
            Test::Is {
                var: var("$X"),
                kind: "int_literal".into(),
            },
        )
    };
    assert_eq!(texts("$X", &[int(), c(false, Test::Nth(-1))]), ["3"]);
    assert_eq!(texts("$X", &[int(), c(false, Test::Nth(2))]), ["2"]);
    let either = Test::Any(vec![c(false, Test::Nth(1)), c(false, Test::Nth(3))]);
    assert_eq!(texts("$X", &[int(), c(false, either)]), ["1", "3"]);
    let both = Test::All(vec![c(false, Test::Nth(1)), c(false, Test::Nth(-1))]);
    assert!(texts("$X", &[int(), c(false, both.clone())]).is_empty());
    assert_eq!(texts("$X", &[int(), c(true, both)]).len(), 3);
    let ident = || {
        c(
            false,
            Test::Is {
                var: var("$F"),
                kind: "identifier".into(),
            },
        )
    };
    let called = texts("$F", &[ident(), c(false, Test::Field("function".into()))]);
    assert_eq!(called, ["g", "use", "use"]);
    let e = constrained("$F", &[c(false, Test::Field("functon".into()))]).unwrap_err();
    assert!(e.contains("did you mean `function`"), "{e}");
    let e = constrained("$F", &[c(false, Test::Nth(0))]).unwrap_err();
    assert!(e.contains("count from 1"), "{e}");
    let e = constrained("$F", &[c(false, Test::Any(Vec::new()))]).unwrap_err();
    assert!(e.contains("at least one"), "{e}");
    // A group checks its members like the top level does.
    let unbound = Test::Any(vec![c(
        false,
        Test::Is {
            var: var("$Q"),
            kind: "identifier".into(),
        },
    )]);
    assert!(constrained("$F", &[c(false, unbound)]).is_err());
}

// ---------- let ------------------------------------------------------

fn derive(var_: &str, from: &str, steps: Vec<colab_core::Transform>) -> Constraint {
    c(
        false,
        Test::Let {
            var: var(var_),
            from: var(from),
            steps,
        },
    )
}

#[test]
fn a_lets_value_keeps_its_base_captures_layout() {
    // `$X` is `$A` upper-cased: many lines, as `$A` is. It is laid out as
    // one token, like a capture, its later lines moving with its line, so
    // the one-per-line call stays one per line; token by token, `$B`'s and
    // `$X`'s text were aligned with each other's, and the call reflowed.
    let lets = [derive("$X", "$A", vec![colab_core::Transform::Upper])];
    let p = StructuralPattern::compile(&rust(), "pair($A, $B)")
        .unwrap()
        .constrain(&rust(), &lets)
        .unwrap();
    let src = "fn f() {\n    pair(\n        build(a,\n              b),\n        c,\n    );\n    pair(x, y);\n}\n";
    assert_eq!(
        p.rewrite(src, "pair($B, $X)"),
        "fn f() {\n    pair(\n        c,\n        BUILD(A,\n              B)\n    );\n    pair(y, X);\n}\n"
    );
}

#[test]
fn a_let_fills_the_template_with_a_derived_value() {
    use colab_core::Transform::{Snake, StripPrefix, Upper};
    let lets = [
        derive("$NAME", "$F", vec![StripPrefix("Get".into())]),
        // From an earlier `let`.
        derive("$SNAKE", "$NAME", vec![Snake, Upper]),
    ];
    let p = constrained("$X.$F()", &lets).unwrap();
    let src = "package p\nfunc f() { u.GetUserID(); u.Other() }\n";
    assert_eq!(
        p.rewrite(src, "$X.$NAME() + $SNAKE"),
        "package p\nfunc f() { u.UserID() + USER_ID; u.Other() + OTHER }\n"
    );
    // Nested matches are rewritten inside out, the `let` with them.
    let nested = "package p\nfunc f() { a.GetB().GetC() }\n";
    assert_eq!(
        p.rewrite(nested, "$X.$NAME()"),
        "package p\nfunc f() { a.B().C() }\n"
    );
    let rule = PatternRule::compile(
        &go(),
        "$X.$F()",
        PatternAction::Replace("$X.$NAME()".into()),
        &lets[..1],
    )
    .unwrap();
    assert_eq!(
        rule.to_string(),
        "pattern '$X.$F()' let $NAME = $F | strip_prefix(\"Get\") -> '$X.$NAME()'"
    );
}

#[test]
fn a_rewriter_rewrites_each_match_inside_a_binding() {
    let rewrite = |pattern: &str, template: &str| {
        colab_core::Transform::Rewrite(vec![colab_core::Rewriter {
            name: "r".into(),
            pattern: pattern.into(),
            template: template.into(),
        }])
    };
    let lets = [derive("$N", "$$$A", vec![rewrite("g($X)", "h($X)")])];
    let p = constrained("f($$$A)", &lets).unwrap();
    let src = "package p\nfunc m() { f(g(1), 2, g(3)); k(g(4)) }\n";
    // Each element the rewriter matches; the rest, and the separators,
    // as they were. Outside the binding nothing changes.
    assert_eq!(
        p.rewrite(src, "ff($N)"),
        "package p\nfunc m() { ff(h(1), 2, h(3)); k(g(4)) }\n"
    );
    // Later steps act on the rewritten text.
    let upper = [derive(
        "$N",
        "$$$A",
        vec![rewrite("g($X)", "h($X)"), colab_core::Transform::Upper],
    )];
    let q = constrained("f($$$A)", &upper).unwrap();
    assert_eq!(
        q.rewrite("package p\nfunc m() { f(g(x)) }\n", "f($N)"),
        "package p\nfunc m() { f(H(X)) }\n"
    );
    let err = |lets: &[Constraint]| constrained("f($$$A)", lets).unwrap_err();
    let late = derive(
        "$N",
        "$$$A",
        vec![colab_core::Transform::Lower, rewrite("g($X)", "h($X)")],
    );
    assert!(err(&[late]).contains("must be the first step"));
    let from_let = [
        derive("$M", "$$$A", vec![colab_core::Transform::Lower]),
        derive("$N", "$M", vec![rewrite("g($X)", "h($X)")]),
    ];
    assert!(err(&from_let).contains("not from a `let`"));
    let unbound = derive("$N", "$$$A", vec![rewrite("g($X)", "h($Y)")]);
    assert!(err(&[unbound]).contains("`$Y`"));
}

#[test]
fn a_let_must_name_a_new_value_from_a_bound_one() {
    let lower = || vec![colab_core::Transform::Lower];
    let err = |lets: &[Constraint]| constrained("$X.$F()", lets).unwrap_err();
    assert!(err(&[derive("$X", "$F", lower())]).contains("already binds `$X`"));
    assert!(err(&[derive("$N", "$Q", lower())]).contains("not a metavariable"));
    assert!(err(&[derive("$$$N", "$F", lower())]).contains("one `$`"));
    assert!(
        err(&[derive("$N", "$F", lower()), derive("$N", "$X", lower())])
            .contains("already a `let`")
    );
    let bad = colab_core::Transform::Replace {
        regex: "(".into(),
        with: String::new(),
    };
    assert!(err(&[derive("$N", "$F", vec![bad])]).contains("replace(/(/"));
    let delete = PatternRule::compile(
        &go(),
        "$X.$F()",
        PatternAction::Delete,
        &[derive("$N", "$F", lower())],
    )
    .unwrap_err();
    assert!(delete.contains("`delete` has none"), "{delete}");
}

// ---------- where constraints ----------------------------------------

fn var(text: &str) -> Var {
    Var::parse(text).unwrap()
}

fn c(negated: bool, test: Test) -> Constraint {
    Constraint { negated, test }
}

fn constrained(pattern: &str, constraints: &[Constraint]) -> Result<StructuralPattern, String> {
    StructuralPattern::compile(&go(), pattern)?.constrain(&go(), constraints)
}

const WRAPS: &str = "package p\n\nfunc a() {\n\terrors.Wrap(err, \"lit\")\n\terrors.Wrap(err, msg)\n\tlog.Println()\n\tlog.Println(1, 2, 3)\n}\n\nfunc TestA(t *testing.T) {\n\terrors.Wrap(err, \"in test\")\n}\n";

fn lines(p: &StructuralPattern) -> Vec<usize> {
    p.find(WRAPS).iter().map(|f| f.line).collect()
}

#[test]
fn matches_filters_on_a_bindings_text() {
    let lit = c(
        false,
        Test::Matches {
            var: var("$M"),
            regex: "^\"".into(),
        },
    );
    let p = constrained("errors.Wrap($E, $M)", std::slice::from_ref(&lit)).unwrap();
    assert_eq!(lines(&p), [4, 11]);
    let not_lit = Constraint {
        negated: true,
        ..lit
    };
    let p = constrained("errors.Wrap($E, $M)", &[not_lit]).unwrap();
    assert_eq!(lines(&p), [5]);
}

#[test]
fn is_filters_on_the_bound_node_kind() {
    let p = constrained(
        "errors.Wrap($E, $M)",
        &[c(
            false,
            Test::Is {
                var: var("$M"),
                kind: "identifier".into(),
            },
        )],
    )
    .unwrap();
    assert_eq!(lines(&p), [5]);
}

#[test]
fn count_counts_named_nodes_in_a_sequence() {
    let count = |min, max| {
        constrained(
            "log.Println($$$A)",
            &[c(
                false,
                Test::Count {
                    var: var("$$$A"),
                    min,
                    max,
                },
            )],
        )
        .unwrap()
    };
    assert_eq!(lines(&count(0, Some(0))), [6]);
    assert_eq!(lines(&count(3, Some(3))), [7]);
    assert_eq!(lines(&count(1, None)), [7]);
}

#[test]
fn inside_and_has_test_ancestors_and_descendants() {
    let in_test = Test::Inside("func $F(t *testing.T) { $$$ }".into());
    let p = constrained("errors.Wrap($E, $M)", &[c(false, in_test.clone())]).unwrap();
    assert_eq!(lines(&p), [11]);
    let p = constrained("errors.Wrap($E, $M)", &[c(true, in_test)]).unwrap();
    assert_eq!(lines(&p), [4, 5]);
    let has = Test::Has("log.Println($$$)".into());
    let p = constrained("func $F() { $$$B }", &[c(false, has)]).unwrap();
    assert_eq!(lines(&p), [3]);
}

#[test]
fn constraints_apply_to_rewrites_too() {
    let p = constrained(
        "errors.Wrap($E, $M)",
        &[c(
            true,
            Test::Inside("func $F(t *testing.T) { $$$ }".into()),
        )],
    )
    .unwrap();
    let out = p.rewrite(WRAPS, "fmt.Errorf($M, $E)");
    assert!(out.contains("errors.Wrap(err, \"in test\")"), "{out}");
    assert_eq!(out.matches("fmt.Errorf").count(), 2, "{out}");
}

#[test]
fn a_constraint_must_name_a_bound_metavariable_with_its_sigil() {
    let matches = |name: &str| {
        c(
            false,
            Test::Matches {
                var: var(name),
                regex: "x".into(),
            },
        )
    };
    let err = constrained("errors.Wrap($E, $M)", &[matches("$X")]).unwrap_err();
    assert!(err.contains("it binds $E, $M"), "{err}");
    let err = constrained("errors.Wrap($E, $M)", &[matches("$$$M")]).unwrap_err();
    assert!(err.contains("written `$M`"), "{err}");
    let count = c(
        false,
        Test::Count {
            var: var("$M"),
            min: 1,
            max: None,
        },
    );
    let err = constrained("errors.Wrap($E, $M)", &[count]).unwrap_err();
    assert!(err.contains("bind it as `$$$M`"), "{err}");
}

#[test]
fn an_unknown_kind_or_bad_regex_fails_at_compile_time() {
    let is = c(
        false,
        Test::Is {
            var: var("$M"),
            kind: "identifer".into(),
        },
    );
    let err = constrained("errors.Wrap($E, $M)", &[is]).unwrap_err();
    assert!(err.contains("did you mean `identifier`"), "{err}");
    let bad = c(
        false,
        Test::Matches {
            var: var("$M"),
            regex: "(".into(),
        },
    );
    assert!(constrained("errors.Wrap($E, $M)", &[bad]).is_err());
    let nested = c(false, Test::Inside("func (".into()));
    let err = constrained("errors.Wrap($E, $M)", &[nested]).unwrap_err();
    assert!(err.starts_with("`inside 'func ('`"), "{err}");
}

fn rust() -> PatternLanguage {
    PatternLanguage {
        grammar: tree_sitter_rust::LANGUAGE.into(),
        expando: 'µ',
        contexts: &["fn f() { @@PATTERN@@ }"],
        also: &[],
        qualified_calls: None,
        leading: Leading::NONE,
        macro_contents: None,
        modifiers: None,
        shorthand_fields: false,
        modifier_tokens: &[],
    }
}

#[test]
fn a_lone_sequence_in_an_item_list_is_a_placeholder() {
    // `$$$B` is not a valid item, but alone in a body it stands for
    // the items.
    let p = StructuralPattern::compile(&rust(), "impl Loader { $$$B }").unwrap();
    let src = "struct Loader;\nimpl Loader {\n    fn load(&self) {}\n}\nimpl Other {}\n";
    let found = p.find(src);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].text.starts_with("impl Loader"));
    // Any other syntax error still fails the pattern.
    assert!(StructuralPattern::compile(&rust(), "impl Loader { fn }").is_err());
}

#[test]
fn a_rewrite_that_only_moves_whitespace_is_not_made() {
    let src = "package p\nfunc g() {\n\tf(\n\t\ta,\n\t\tb,\n\t)\n}\n";
    let p = StructuralPattern::compile(&go(), "f($$$A)").unwrap();
    assert_eq!(
        p.rewrite(src, "f($$$A)"),
        src,
        "an identity rewrite reflowed the call"
    );
    assert_ne!(p.rewrite(src, "h($$$A)"), src);
    // Whitespace inside a string is content.
    assert!(!same_but_whitespace(r#"f("a b")"#, r#"f("ab")"#));
    assert!(same_but_whitespace("f(\n a,\n b)", "f(a, b)"));
}

#[test]
fn a_match_keeps_no_children_the_pattern_leaves_out() {
    // An `else` is a trailing child `if $C { $$$B }` does not mention:
    // rewriting that match would drop it.
    let p = StructuralPattern::compile(&go(), "if $C { $$$B }").unwrap();
    let src = "package p\n\nfunc f() {\n\tif a {\n\t\tx()\n\t}\n\tif b {\n\t\ty()\n\t} else {\n\t\tz()\n\t}\n}\n";
    let found = p.find(src);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].text.starts_with("if a"));
    // A `$$$` still takes any number of children.
    let q = StructuralPattern::compile(&go(), "f($$$A)").unwrap();
    assert_eq!(q.find("package p\nfunc g() { f(1, 2, 3) }\n").len(), 1);
}

#[test]
fn an_also_context_matches_the_same_text_parsed_another_way() {
    // `Optional[str]` is a `subscript` in an expression and a
    // `generic_type` in an annotation: one pattern finds both.
    let p = StructuralPattern::compile(&python(), "Optional[$T]").unwrap();
    let src = "def f(a: Optional[str]) -> Optional[int]:\n    return cast(Optional[str], a)\n";
    assert_eq!(p.find(src).len(), 3);
    assert_eq!(
        p.rewrite(src, "$T | None"),
        "def f(a: str | None) -> int | None:\n    return cast(str | None, a)\n"
    );
}

#[test]
fn a_multi_line_template_follows_tab_indentation() {
    let go = PatternLanguage {
        grammar: tree_sitter_go::LANGUAGE.into(),
        expando: 'µ',
        contexts: &["package p\nfunc _() {\n@@PATTERN@@\n}\n"],
        also: &[],
        qualified_calls: None,
        leading: Leading::NONE,
        macro_contents: None,
        modifiers: None,
        shorthand_fields: false,
        modifier_tokens: &[],
    };
    let p = StructuralPattern::compile(&go, "log.Print($A)").unwrap();
    let src = "package p\n\nfunc f() {\n\tif x {\n\t\tlog.Print(a)\n\t}\n}\n";
    let out = p.rewrite(src, "if debug {\n\tlog.Print($A)\n}");
    assert_eq!(
        out,
        "package p\n\nfunc f() {\n\tif x {\n\t\tif debug {\n\t\t\tlog.Print(a)\n\t\t}\n\t}\n}\n"
    );
}

#[test]
fn a_multi_line_template_leaves_no_trailing_whitespace() {
    // ast-grep re-indents a template's later lines to the match's
    // column (spaces only; tab-indented Go is left flush-left).
    let p = StructuralPattern::compile(&python(), "print($$$A)").unwrap();
    let out = p.rewrite("def f():\n    print(1)\n", "print($$$A)\n\nprint(2)");
    assert_eq!(out, "def f():\n    print(1)\n\n    print(2)\n");
}

#[test]
fn expand_rewrites_only_metavariables() {
    assert_eq!(expand("f($A, $$$REST, $_)", 'µ'), "f(µA, µµµREST, µ_)");
    assert_eq!(expand("echo $x; \"$5\"", 'µ'), "echo $x; \"$5\"");
    assert_eq!(expand("{ $$$ }", 'µ'), "{ µµµ }");
}

#[test]
fn go_expression_compiles_through_a_context() {
    let p = StructuralPattern::compile(&go(), "errors.Wrap($ERR, $MSG)").unwrap();
    assert_eq!(p.prefilter(), Some("errors"));
    let found = p.find(GO_SRC);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].line, 5);
    assert_eq!(
        found[0].captures,
        vec![
            ("ERR".to_string(), "err".to_string()),
            ("MSG".to_string(), "\"f failed\"".to_string())
        ]
    );
}

fn qualified_rust() -> PatternLanguage {
    PatternLanguage {
        qualified_calls: Some("::"),
        ..rust()
    }
}

const CALLS_SRC: &str = "fn f() {\n    to_svg(a, b);\n    svg::to_svg(c, d);\n    \
                         diagram_scene::svg::to_svg(e, f);\n    x.to_svg(g, h);\n}\n";

#[test]
fn a_bare_call_pattern_also_finds_qualified_calls() {
    let p = StructuralPattern::compile(&qualified_rust(), "to_svg($A, $B)").unwrap();
    let found: Vec<_> = p.find(CALLS_SRC).into_iter().map(|f| f.text).collect();
    assert_eq!(
        found,
        [
            "to_svg(a, b)",
            "svg::to_svg(c, d)",
            "diagram_scene::svg::to_svg(e, f)"
        ],
        "the method call is not a call of the function"
    );
    // A user metavariable binds the same as before; the path's is internal.
    let qualified = &p.find(CALLS_SRC)[1];
    assert!(qualified.captures.contains(&("A".into(), "c".into())));
}

#[test]
fn a_rewrite_keeps_the_path_it_matched() {
    let p = StructuralPattern::compile(&qualified_rust(), "to_svg($A, $B)").unwrap();
    let renamed = p.rewrite(CALLS_SRC, "render($A, &$B)");
    assert!(renamed.contains("    render(a, &b);"), "{renamed}");
    assert!(renamed.contains("    svg::render(c, &d);"), "{renamed}");
    assert!(
        renamed.contains("diagram_scene::svg::render(e, &f);"),
        "{renamed}"
    );
    assert!(renamed.contains("x.to_svg(g, h)"), "{renamed}");
    assert_eq!(p.rewrite(&renamed, "render($A, &$B)"), renamed);
    // A wrap keeps the call as matched inside it only when bare.
    let method = p.rewrite(CALLS_SRC, "$A.to_svg($B)");
    assert!(method.contains("    c.to_svg(d);"), "{method}");
}

#[test]
fn only_a_bare_callee_is_qualified() {
    for text in [
        "$F($A)",
        "svg::to_svg($A)",
        "println!($A)",
        "if ($A) {}",
        "x.f($A)",
    ] {
        assert!(qualify_call(text, "::").is_none(), "{text}");
    }
    assert_eq!(
        qualify_call("parse::<T>($A)", "::").map(|q| q.0),
        Some("$COLAB_PATH::parse::<T>($A)".into())
    );
    // Without the separator set, nothing changes.
    let p = StructuralPattern::compile(&rust(), "to_svg($A, $B)").unwrap();
    assert_eq!(p.find(CALLS_SRC).len(), 1);
}

#[test]
fn rewrite_substitutes_bindings() {
    let p = StructuralPattern::compile(&go(), "errors.Wrap($ERR, $MSG)").unwrap();
    let out = p.rewrite(GO_SRC, "fmt.Errorf($MSG+\": %w\", $ERR)");
    assert!(
        out.contains("return fmt.Errorf(\"f failed\"+\": %w\", err)"),
        "{out}"
    );
    assert_eq!(p.rewrite(&out, "x"), out, "nothing left to match");
}

#[test]
fn multi_capture_spans_all_arguments() {
    let p = StructuralPattern::compile(&go(), "log.Println($$$ARGS)").unwrap();
    let found = p.find(GO_SRC);
    assert_eq!(
        found[0].captures,
        vec![("ARGS".to_string(), "\"a\", 1".to_string())]
    );
    let out = p.rewrite(GO_SRC, "slog.Info($$$ARGS)");
    assert!(out.contains("slog.Info(\"a\", 1)"), "{out}");
}

#[test]
fn nested_matches_are_rewritten_in_one_pass() {
    let p = StructuralPattern::compile(&go(), "errors.Wrap($ERR, $MSG)").unwrap();
    let src = "package main\n\nfunc f() error {\n\treturn errors.Wrap(errors.Wrap(err, \"a\"), \"b\")\n}\n";
    let tpl = "fmt.Errorf(\"%s: %w\", $MSG, $ERR)";
    let out = p.rewrite(src, tpl);
    assert!(
        out.contains("return fmt.Errorf(\"%s: %w\", \"b\", fmt.Errorf(\"%s: %w\", \"a\", err))"),
        "{out}"
    );
    assert_eq!(p.rewrite(&out, tpl), out, "idempotent");
}

#[test]
fn nested_multi_captures_and_wrap_templates_rewrite_each_original_match_once() {
    let p = StructuralPattern::compile(&go(), "f($$$A)").unwrap();
    let src = "package main\n\nfunc main() {\n\tf(1, f(2))\n}\n";
    let out = p.rewrite(src, "f(g($$$A))");
    assert!(out.contains("f(g(1, f(g(2))))"), "{out}");
}

#[test]
fn an_unwrapping_template_is_only_metavariables() {
    assert!(only_metavariables("$$$B"));
    assert!(only_metavariables("  $A\n$$$REST "));
    assert!(!only_metavariables("fn new() {}"));
    assert!(!only_metavariables("pub $$$B"));
    assert!(!only_metavariables(""));
}

#[test]
fn template_dollars_that_are_not_metavariables_survive() {
    assert_eq!(
        transformable_template("$this->x($$$ARGS, $A, \"$5\", $$)"),
        (
            "$this->x($ARGS, $A, \"$5\", $$)".to_string(),
            vec!["ARGS".to_string(), "A".to_string()]
        )
    );
}

#[test]
fn repeated_metavariable_must_bind_equal_text() {
    let p = StructuralPattern::compile(&python(), "$A == $A").unwrap();
    assert_eq!(p.find("x == x\nx == y\n").len(), 1);
}

#[test]
fn delete_removes_whole_statement_lines() {
    let p = StructuralPattern::compile(&go(), "log.Println($$$ARGS)").unwrap();
    let out = p.delete(GO_SRC);
    assert!(!out.contains("log.Println"));
    assert!(!out.contains("\n\t\n"), "no blank line left: {out:?}");
}

#[test]
fn delete_takes_a_list_elements_comma_and_leaves_other_nested_matches() {
    let p = StructuralPattern::compile(&python(), "print($X)").unwrap();
    let src = "print(0)\ny = [print(1), 2]\nz = [3, print(4)]\nn = len(print(5))\n";
    assert_eq!(p.delete(src), "y = [2]\nz = [3]\nn = len(print(5))\n");
    // Only the trailing comma when it is the sole element; nested
    // matches inside a removed element go with it.
    let q = StructuralPattern::compile(&go(), "f($$$A)").unwrap();
    let go_src = "package p\nfunc g() { h(f(1,), 2); h(f(f(3)), 4) }\n";
    assert_eq!(q.delete(go_src), "package p\nfunc g() { h(2); h(4) }\n");
    let sole = "package p\nfunc g() { h(f(1,)) }\n";
    assert_eq!(q.delete(sole), sole);
}

#[test]
fn a_rewrite_that_would_break_the_syntax_is_not_applied() {
    let rule = PatternRule::compile(
        &python(),
        "foo($A)",
        PatternAction::Replace("bar($A".into()),
        &[],
    )
    .unwrap();
    let src = "x = foo(1)\n";
    assert_eq!(rule.apply(src), src);
    // A sound template on the same input still applies.
    let ok = PatternRule::compile(
        &python(),
        "foo($A)",
        PatternAction::Replace("bar($A)".into()),
        &[],
    )
    .unwrap();
    assert_eq!(ok.apply(src), "x = bar(1)\n");
}

#[test]
fn an_already_broken_file_is_still_rewritten() {
    // The guard protects clean files; it must not freeze files that
    // were already unparseable (tree-sitter recovers around errors).
    let ok = PatternRule::compile(
        &python(),
        "foo($A)",
        PatternAction::Replace("bar($A)".into()),
        &[],
    )
    .unwrap();
    assert_eq!(ok.apply("x = foo(1)\ndef (\n"), "x = bar(1)\ndef (\n");
}

#[test]
fn invalid_pattern_names_every_attempt() {
    let err = StructuralPattern::compile(&go(), "func (").unwrap_err();
    assert!(err.contains("as-is"), "{err}");
    assert!(err.contains("func _()"), "{err}");
}

#[test]
fn python_statement_pattern_needs_no_context() {
    let p = StructuralPattern::compile(&python(), "self.assertEquals($A, $B)").unwrap();
    let out = p.rewrite(
        "class T:\n    def test(self):\n        self.assertEquals(f(1), 2)\n",
        "self.assertEqual($A, $B)",
    );
    assert!(out.contains("self.assertEqual(f(1), 2)"), "{out}");
    assert_eq!(p.prefilter(), Some("assertEquals"));
}

#[test]
fn all_metavariable_pattern_has_no_prefilter() {
    let p = StructuralPattern::compile(&python(), "$A + $B").unwrap();
    assert_eq!(p.prefilter(), None);
}

#[test]
fn pattern_tree_shows_metavariables_and_the_context_used() {
    let p = StructuralPattern::compile(&go(), "errors.Wrap($ERR, $MSG)").unwrap();
    assert!(p.context().unwrap().contains("func _()"));
    let tree = p.tree();
    assert!(tree.starts_with("call_expression"), "{tree}");
    assert!(tree.contains("\"$ERR\""), "{tree}");
    assert!(tree.contains("field: field_identifier"), "{tree}");
}

#[test]
fn syntax_tree_reports_errors() {
    let grammar: tree_sitter::Language = tree_sitter_go::LANGUAGE.into();
    let (tree, has_error) = syntax_tree(&grammar, "package p\nfunc f() { x( }\n").unwrap();
    assert!(has_error);
    assert!(tree.contains("ERROR") || tree.contains("MISSING"), "{tree}");
    let (_, has_error) = syntax_tree(&grammar, "package p\n").unwrap();
    assert!(!has_error);
}

#[test]
fn bare_pattern_tree_starts_at_the_matched_node() {
    let p = StructuralPattern::compile(&python(), "foo.bar($A)").unwrap();
    assert!(p.context().is_none());
    assert!(p.tree().starts_with("call"), "{}", p.tree());
}

#[test]
fn a_context_whose_wrapper_shadows_the_pattern_kind_is_refused() {
    // `x` alone selects an `identifier`; in Go's function-body wrapper the
    // function name `_` is an earlier `identifier`, so using that context
    // would silently match `_` instead. It must be refused, not used.
    let err = StructuralPattern::compile(&go(), "x").unwrap_err();
    assert!(err.contains("earlier `identifier`"), "{err}");
}

#[test]
fn compiled_pattern_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<StructuralPattern>();
}
