//! What the common floor asks of Rust beyond its tree, answered from
//! rust-analyzer's typed AST ([`colab_rewrite::language::OwnSyntax`]):
//! declarations (`colab edit --symbol` without a server, the self-call
//! check) and calls (`rust::call`).
//!
//! Both keep what tree-sitter gave: a declaration's kind is tree-sitter's
//! name for it (`function_item`, `impl_item`, …), named as before (an
//! `impl` by its type), its range widened to the doc comments and
//! attributes directly above it; a call's callee is the text before its
//! arguments, so a method call is `x.m` and a turbofish stays
//! (`f::<T>`), as when both were tree-sitter's `call_expression`.

use std::ops::Range;

use colab_core::symbols::Declaration;
use colab_parse_rust::ra::ast::{self, AstNode, HasArgList, HasName};
use colab_parse_rust::ra::{NodeOrToken, SyntaxElement, SyntaxKind, SyntaxNode, TextRange};
use colab_rewrite::language::{Call, OwnSyntax};

/// Rust's answers.
pub struct RustSyntax;

/// The one provider.
pub static OWN: RustSyntax = RustSyntax;

fn range(r: TextRange) -> Range<usize> {
    usize::from(r.start())..usize::from(r.end())
}

fn root(source: &str) -> SyntaxNode {
    colab_parse_rust::source_file(source).0.syntax_node()
}

impl OwnSyntax for RustSyntax {
    fn declarations(&self, source: &str) -> Vec<Declaration> {
        let mut out = Vec::new();
        collect(&root(source), source, &mut out);
        out
    }

    fn calls(&self, source: &str) -> Vec<Call> {
        root(source)
            .descendants()
            .filter_map(|node| call(&node, source))
            .collect()
    }
}

/// `node` as a call: a call expression or a method call, its callee the
/// text before its argument list.
fn call(node: &SyntaxNode, source: &str) -> Option<Call> {
    let args = match node.kind() {
        SyntaxKind::CALL_EXPR => ast::CallExpr::cast(node.clone())?.arg_list()?,
        SyntaxKind::METHOD_CALL_EXPR => ast::MethodCallExpr::cast(node.clone())?.arg_list()?,
        _ => return None,
    };
    let whole = range(node.text_range());
    let list = range(args.syntax().text_range());
    let callee = source[whole.start..list.start].trim_end().to_string();
    let arguments = args
        .args()
        .map(|a| source[range(a.syntax().text_range())].to_string())
        .collect();
    Some(Call {
        range: whole,
        callee,
        arguments,
    })
}

/// Declarations at or below `node`.
fn collect(node: &SyntaxNode, source: &str, out: &mut Vec<Declaration>) {
    for child in node.children() {
        match declaration(&child, source) {
            Some(mut d) => {
                collect(&child, source, &mut d.children);
                out.push(d);
            }
            None => collect(&child, source, out),
        }
    }
}

/// The last identifier in `text`: `Config` in `Box<Config>` or `&mut Config`.
fn last_word(text: &str) -> String {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .rfind(|w| !w.is_empty() && w.chars().next().is_some_and(|c| !c.is_ascii_digit()))
        .unwrap_or("")
        .to_string()
}

fn declaration(node: &SyntaxNode, source: &str) -> Option<Declaration> {
    let named = |n: Option<ast::Name>| n.map(|n| n.syntax().text().to_string());
    let (kind, name) = match node.kind() {
        SyntaxKind::FN => {
            let f = ast::Fn::cast(node.clone())?;
            let kind = if f.body().is_some() {
                "function_item"
            } else {
                "function_signature_item"
            };
            (kind, named(f.name()))
        }
        SyntaxKind::STRUCT => (
            "struct_item",
            named(ast::Struct::cast(node.clone())?.name()),
        ),
        SyntaxKind::ENUM => ("enum_item", named(ast::Enum::cast(node.clone())?.name())),
        SyntaxKind::VARIANT => (
            "enum_variant",
            named(ast::Variant::cast(node.clone())?.name()),
        ),
        SyntaxKind::UNION => ("union_item", named(ast::Union::cast(node.clone())?.name())),
        SyntaxKind::TRAIT => ("trait_item", named(ast::Trait::cast(node.clone())?.name())),
        SyntaxKind::MODULE => ("mod_item", named(ast::Module::cast(node.clone())?.name())),
        SyntaxKind::CONST => ("const_item", named(ast::Const::cast(node.clone())?.name())),
        SyntaxKind::STATIC => (
            "static_item",
            named(ast::Static::cast(node.clone())?.name()),
        ),
        SyntaxKind::MACRO_RULES => (
            "macro_definition",
            named(ast::MacroRules::cast(node.clone())?.name()),
        ),
        SyntaxKind::TYPE_ALIAS => {
            let t = ast::TypeAlias::cast(node.clone())?;
            // A trait's `type Item;` is tree-sitter's `associated_type`,
            // which is no declaration.
            t.ty()?;
            ("type_item", named(t.name()))
        }
        SyntaxKind::IMPL => {
            let i = ast::Impl::cast(node.clone())?;
            (
                "impl_item",
                i.self_ty()
                    .map(|t| last_word(&t.syntax().text().to_string())),
            )
        }
        _ => return None,
    };
    let name = name?.trim().to_string();
    if name.is_empty() {
        return None;
    }
    Some(Declaration {
        name,
        kind: kind.to_string(),
        range: extent(node, source),
        children: Vec::new(),
    })
}

/// Whether `element` is one of an item's leading doc comments, comments
/// or attributes: what goes with it when directly above it.
fn leading(element: &SyntaxElement) -> bool {
    match element {
        NodeOrToken::Node(n) => matches!(n.kind(), SyntaxKind::ATTR | SyntaxKind::DOC_COMMENT),
        NodeOrToken::Token(t) => matches!(
            t.kind(),
            SyntaxKind::COMMENT | SyntaxKind::OUTER_DOC_COMMENT | SyntaxKind::INNER_DOC_COMMENT
        ),
    }
}

fn text_of(element: &SyntaxElement) -> String {
    match element {
        NodeOrToken::Node(n) => n.text().to_string(),
        NodeOrToken::Token(t) => t.text().to_string(),
    }
}

/// `node`'s bytes from its own first token (after the attributes and
/// comments rust-analyzer holds inside it), widened to the comments and
/// attributes directly above it: inside it or before it, no blank line
/// between, and no inner doc or attribute (`//!`, `#![…]`), which belongs
/// to what encloses it.
fn extent(node: &SyntaxNode, source: &str) -> Range<usize> {
    let whole = range(node.text_range());
    // Its leading elements, then its siblings before it, nearest first.
    let mut before: Vec<SyntaxElement> = Vec::new();
    let mut own_start = whole.start;
    for element in node.children_with_tokens() {
        if element.kind() == SyntaxKind::WHITESPACE {
            continue;
        }
        if leading(&element) {
            before.push(element);
        } else {
            own_start = range(element.text_range()).start;
            break;
        }
    }
    before.reverse();
    let mut sibling = node.prev_sibling_or_token();
    while let Some(s) = sibling {
        sibling = s.prev_sibling_or_token();
        if s.kind() != SyntaxKind::WHITESPACE {
            before.push(s);
        }
    }
    let mut start = own_start;
    for element in before {
        let text = text_of(&element);
        let inner = text.starts_with("//!") || text.starts_with("/*!") || text.starts_with("#!");
        let r = range(element.text_range());
        let gap = &source[r.end..start];
        if inner || !leading(&element) || gap.matches('\n').count() > 1 || !gap.trim().is_empty() {
            break;
        }
        start = r.start;
    }
    start..whole.end
}

#[cfg(test)]
mod tests {
    use super::*;
    use colab_core::symbols::{flatten, resolve};

    fn paths(source: &str) -> Vec<String> {
        let decls = OWN.declarations(source);
        flatten(&decls).iter().map(|f| f.path.join("/")).collect()
    }

    #[test]
    fn impls_are_named_by_their_type_and_take_their_attributes() {
        let src = "//! The crate.\nmod m;\nstruct Config;\n\n/// Docs.\n#[inline]\nfn top() {}\n\nimpl Display for Config {\n    fn fmt(&self) {}\n}\n\nmod inner {\n    fn deep() {}\n}\n";
        assert_eq!(
            paths(src),
            [
                "m",
                "Config",
                "top",
                "Config",
                "Config/fmt",
                "inner",
                "inner/deep"
            ]
        );
        let all = OWN.declarations(src);
        let m = resolve(&all, "m", src).unwrap();
        assert_eq!(&src[m.declaration.range.clone()], "mod m;");
        let top = resolve(&all, "top", src).unwrap();
        assert!(src[top.declaration.range.clone()].starts_with("/// Docs.\n#[inline]\nfn top"));
        assert!(resolve(&all, "Config/fmt", src).is_ok());
    }

    #[test]
    fn a_blank_line_or_an_inner_doc_ends_what_goes_with_an_item() {
        let src = "// unrelated\n\n/// Docs.\nfn f() {}\n";
        let all = OWN.declarations(src);
        let f = resolve(&all, "f", src).unwrap();
        assert_eq!(&src[f.declaration.range.clone()], "/// Docs.\nfn f() {}");
    }

    #[test]
    fn calls_and_method_calls_are_named_by_the_text_before_their_arguments() {
        let src = "fn f() { a::b(1, g(2)); x.m::<T>(y); }\n";
        let calls: Vec<(String, Vec<String>)> = OWN
            .calls(src)
            .into_iter()
            .map(|c| (c.callee, c.arguments))
            .collect();
        assert_eq!(
            calls,
            [
                (
                    "a::b".to_string(),
                    vec!["1".to_string(), "g(2)".to_string()]
                ),
                ("g".to_string(), vec!["2".to_string()]),
                ("x.m::<T>".to_string(), vec!["y".to_string()]),
            ]
        );
    }
}
