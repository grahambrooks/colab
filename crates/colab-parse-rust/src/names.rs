//! tree-sitter's names on rust-analyzer's trees, so rules written for
//! colab's tree-sitter Rust keep working: kind aliases (`where $X is
//! call_expression`) and fields (`field function`, what `generalize` keeps
//! literal). rust-analyzer's own kind names stay the tree's, for the finer
//! distinctions it draws (`call_expr` and `method_call_expr`).
//!
//! Both tables come from lining up the two parsers' nodes over the same
//! text in the Cargo registry (`colab-backends/tests/rust_parser.rs`,
//! `align_tree_sitter_and_rust_analyzer_names`). An alias is kept where
//! the constructs are the same, even if rust-analyzer's kind is a little
//! wider (`binary_expression` is every `bin_expr`, assignments too); a
//! tree-sitter name with no counterpart (`boolean_literal`,
//! `type_identifier`) is left out, so a rule spelling it fails with a
//! "did you mean" rather than matching more than it did.

use std::collections::HashMap;

use ra_ap_syntax::ast::{
    self, AstNode, HasArgList, HasGenericArgs, HasGenericParams, HasLoopBody, HasName,
};
use ra_ap_syntax::{SyntaxKind, SyntaxNode};

/// tree-sitter's Rust kind → the rust-analyzer kinds it stands for.
pub const ALIASES: &[(&str, &[&str])] = &[
    ("abstract_type", &["impl_trait_type"]),
    ("arguments", &["arg_list"]),
    ("array_expression", &["array_expr"]),
    ("array_type", &["array_type", "slice_type"]),
    ("assignment_expression", &["bin_expr"]),
    ("async_block", &["block_expr"]),
    ("attribute_item", &["attr"]),
    ("await_expression", &["await_expr"]),
    ("binary_expression", &["bin_expr"]),
    ("block", &["block_expr", "stmt_list"]),
    ("block_comment", &["comment", "doc_comment"]),
    ("break_expression", &["break_expr"]),
    ("call_expression", &["call_expr", "method_call_expr"]),
    ("char_literal", &["char", "byte"]),
    ("closure_expression", &["closure_expr"]),
    ("closure_parameters", &["param_list"]),
    ("compound_assignment_expr", &["bin_expr"]),
    ("const_block", &["block_expr"]),
    ("const_item", &["const"]),
    ("const_parameter", &["const_param"]),
    ("continue_expression", &["continue_expr"]),
    (
        "declaration_list",
        &["assoc_item_list", "extern_item_list", "item_list"],
    ),
    ("dynamic_type", &["dyn_trait_type"]),
    ("enum_item", &["enum"]),
    ("enum_variant", &["variant"]),
    ("enum_variant_list", &["variant_list"]),
    ("expression_statement", &["expr_stmt"]),
    ("extern_crate_declaration", &["extern_crate"]),
    ("field_declaration", &["record_field"]),
    ("field_declaration_list", &["record_field_list"]),
    ("field_expression", &["field_expr"]),
    ("field_initializer", &["record_expr_field"]),
    ("field_initializer_list", &["record_expr_field_list"]),
    ("field_pattern", &["record_pat_field"]),
    ("float_literal", &["float_number"]),
    ("for_expression", &["for_expr"]),
    ("foreign_mod_item", &["extern_block"]),
    ("function_item", &["fn"]),
    ("function_signature_item", &["fn"]),
    ("function_type", &["fn_ptr_type"]),
    ("identifier", &["ident"]),
    ("if_expression", &["if_expr"]),
    ("impl_item", &["impl"]),
    ("index_expression", &["index_expr"]),
    ("inner_attribute_item", &["attr"]),
    ("integer_literal", &["int_number"]),
    ("let_condition", &["let_expr"]),
    ("let_declaration", &["let_stmt"]),
    ("lifetime_parameter", &["lifetime_param"]),
    ("line_comment", &["comment", "doc_comment"]),
    ("loop_expression", &["loop_expr"]),
    ("macro_definition", &["macro_rules"]),
    ("macro_invocation", &["macro_call"]),
    ("match_arm", &["match_arm"]),
    ("match_block", &["match_arm_list"]),
    ("match_expression", &["match_expr"]),
    ("mod_item", &["module"]),
    ("or_pattern", &["or_pat"]),
    ("ordered_field_declaration_list", &["tuple_field_list"]),
    ("parameter", &["param"]),
    ("parameters", &["param_list"]),
    ("parenthesized_expression", &["paren_expr"]),
    ("pointer_type", &["ptr_type"]),
    ("range_expression", &["range_expr"]),
    ("range_pattern", &["range_pat"]),
    ("raw_string_literal", &["string", "byte_string", "c_string"]),
    ("reference_expression", &["ref_expr"]),
    ("reference_pattern", &["ref_pat"]),
    ("reference_type", &["ref_type"]),
    ("return_expression", &["return_expr"]),
    ("scoped_identifier", &["path"]),
    ("scoped_type_identifier", &["path"]),
    ("scoped_use_list", &["use_tree"]),
    ("self_parameter", &["self_param"]),
    ("slice_pattern", &["slice_pat"]),
    ("static_item", &["static"]),
    ("string_literal", &["string", "byte_string", "c_string"]),
    ("struct_expression", &["record_expr"]),
    ("struct_item", &["struct"]),
    ("struct_pattern", &["record_pat"]),
    ("trait_item", &["trait"]),
    ("try_expression", &["try_expr"]),
    ("tuple_expression", &["tuple_expr"]),
    ("tuple_pattern", &["tuple_pat"]),
    ("tuple_struct_pattern", &["tuple_struct_pat"]),
    ("type_arguments", &["generic_arg_list"]),
    ("type_binding", &["assoc_type_arg"]),
    ("type_cast_expression", &["cast_expr"]),
    ("type_item", &["type_alias"]),
    ("type_parameter", &["type_param"]),
    ("type_parameters", &["generic_param_list"]),
    ("unary_expression", &["prefix_expr"]),
    ("union_item", &["union"]),
    ("unit_expression", &["tuple_expr"]),
    ("unit_type", &["tuple_type"]),
    ("unsafe_block", &["block_expr"]),
    ("use_as_clause", &["use_tree"]),
    ("use_declaration", &["use"]),
    ("use_list", &["use_tree_list"]),
    ("use_wildcard", &["use_tree"]),
    ("visibility_modifier", &["visibility"]),
    ("where_predicate", &["where_pred"]),
    ("while_expression", &["while_expr"]),
];

/// The field names the adapter gives children, tree-sitter's.
pub const FIELDS: &[&str] = &[
    "",
    "alias",
    "alternative",
    "argument",
    "arguments",
    "body",
    "condition",
    "consequence",
    "element",
    "field",
    "function",
    "left",
    "length",
    "list",
    "macro",
    "name",
    "operator",
    "parameters",
    "path",
    "pattern",
    "return_type",
    "right",
    "trait",
    "type",
    "type_arguments",
    "type_parameters",
    "value",
];

/// A child, by kind and range, and the field it is in.
pub type Fields = HashMap<(SyntaxKind, u32, u32), &'static str>;

fn key(node: &SyntaxNode) -> (SyntaxKind, u32, u32) {
    let r = node.text_range();
    (node.kind(), r.start().into(), r.end().into())
}

/// The fields of every child under `root` that tree-sitter's Rust names.
pub fn fields(root: &SyntaxNode) -> Fields {
    let mut out = Fields::new();
    let mut put = |node: Option<SyntaxNode>, field: &'static str| {
        if let Some(n) = node {
            out.entry(key(&n)).or_insert(field);
        }
    };
    fn s<N: AstNode>(n: Option<N>) -> Option<SyntaxNode> {
        n.map(|n| n.syntax().clone())
    }
    for node in root.descendants() {
        // Anything named: its name.
        if let Some(named) = ast::AnyHasName::cast(node.clone()) {
            put(s(named.name()), "name");
        }
        if let Some(g) = ast::AnyHasGenericParams::cast(node.clone()) {
            put(s(g.generic_param_list()), "type_parameters");
        }
        if let Some(a) = ast::AnyHasArgList::cast(node.clone()) {
            put(s(a.arg_list()), "arguments");
        }
        match node.kind() {
            SyntaxKind::FN => {
                let f = ast::Fn::cast(node).unwrap_or_else(|| unreachable!());
                put(s(f.param_list()), "parameters");
                put(s(f.ret_type().and_then(|r| r.ty())), "return_type");
                put(s(f.body()), "body");
            }
            SyntaxKind::CLOSURE_EXPR => {
                let c = ast::ClosureExpr::cast(node).unwrap_or_else(|| unreachable!());
                put(s(c.param_list()), "parameters");
                put(s(c.ret_type().and_then(|r| r.ty())), "return_type");
                put(s(c.body()), "body");
            }
            SyntaxKind::CALL_EXPR => {
                let c = ast::CallExpr::cast(node).unwrap_or_else(|| unreachable!());
                put(s(c.expr()), "function");
            }
            SyntaxKind::FIELD_EXPR => {
                let f = ast::FieldExpr::cast(node).unwrap_or_else(|| unreachable!());
                put(s(f.expr()), "value");
                put(s(f.name_ref()), "field");
            }
            SyntaxKind::RECORD_EXPR => {
                let r = ast::RecordExpr::cast(node).unwrap_or_else(|| unreachable!());
                put(s(r.path()), "name");
                put(s(r.record_expr_field_list()), "body");
            }
            SyntaxKind::RECORD_EXPR_FIELD => {
                let f = ast::RecordExprField::cast(node).unwrap_or_else(|| unreachable!());
                put(s(f.name_ref()), "field");
                put(s(f.expr()), "value");
            }
            SyntaxKind::LET_STMT => {
                let l = ast::LetStmt::cast(node).unwrap_or_else(|| unreachable!());
                put(s(l.pat()), "pattern");
                put(s(l.ty()), "type");
                put(s(l.initializer()), "value");
                put(s(l.let_else().and_then(|e| e.block_expr())), "alternative");
            }
            SyntaxKind::LET_EXPR => {
                let l = ast::LetExpr::cast(node).unwrap_or_else(|| unreachable!());
                put(s(l.pat()), "pattern");
                put(s(l.expr()), "value");
            }
            SyntaxKind::BIN_EXPR => {
                let b = ast::BinExpr::cast(node).unwrap_or_else(|| unreachable!());
                put(s(b.lhs()), "left");
                put(s(b.rhs()), "right");
            }
            SyntaxKind::IF_EXPR => {
                let i = ast::IfExpr::cast(node).unwrap_or_else(|| unreachable!());
                put(s(i.condition()), "condition");
                put(s(i.then_branch()), "consequence");
            }
            SyntaxKind::WHILE_EXPR => {
                let w = ast::WhileExpr::cast(node).unwrap_or_else(|| unreachable!());
                put(s(w.condition()), "condition");
                put(s(w.loop_body()), "body");
            }
            SyntaxKind::FOR_EXPR => {
                let f = ast::ForExpr::cast(node).unwrap_or_else(|| unreachable!());
                put(s(f.pat()), "pattern");
                put(s(f.iterable()), "value");
                put(s(f.loop_body()), "body");
            }
            SyntaxKind::LOOP_EXPR => {
                let l = ast::LoopExpr::cast(node).unwrap_or_else(|| unreachable!());
                put(s(l.loop_body()), "body");
            }
            SyntaxKind::MATCH_EXPR => {
                let m = ast::MatchExpr::cast(node).unwrap_or_else(|| unreachable!());
                put(s(m.expr()), "value");
                put(s(m.match_arm_list()), "body");
            }
            SyntaxKind::MATCH_ARM => {
                let a = ast::MatchArm::cast(node).unwrap_or_else(|| unreachable!());
                put(s(a.pat()), "pattern");
                put(s(a.expr()), "value");
            }
            SyntaxKind::PARAM => {
                let p = ast::Param::cast(node).unwrap_or_else(|| unreachable!());
                put(s(p.pat()), "pattern");
                put(s(p.ty()), "type");
            }
            SyntaxKind::REF_EXPR => {
                let r = ast::RefExpr::cast(node).unwrap_or_else(|| unreachable!());
                put(s(r.expr()), "value");
            }
            SyntaxKind::REF_TYPE => {
                let r = ast::RefType::cast(node).unwrap_or_else(|| unreachable!());
                put(s(r.ty()), "type");
            }
            SyntaxKind::PTR_TYPE => {
                let p = ast::PtrType::cast(node).unwrap_or_else(|| unreachable!());
                put(s(p.ty()), "type");
            }
            SyntaxKind::CAST_EXPR => {
                let c = ast::CastExpr::cast(node).unwrap_or_else(|| unreachable!());
                put(s(c.expr()), "value");
                put(s(c.ty()), "type");
            }
            SyntaxKind::ARRAY_TYPE => {
                let a = ast::ArrayType::cast(node).unwrap_or_else(|| unreachable!());
                put(s(a.ty()), "element");
                put(s(a.const_arg()), "length");
            }
            SyntaxKind::SLICE_TYPE => {
                let a = ast::SliceType::cast(node).unwrap_or_else(|| unreachable!());
                put(s(a.ty()), "element");
            }
            SyntaxKind::CONST => {
                let c = ast::Const::cast(node).unwrap_or_else(|| unreachable!());
                put(s(c.ty()), "type");
                put(s(c.body()), "value");
            }
            SyntaxKind::STATIC => {
                let c = ast::Static::cast(node).unwrap_or_else(|| unreachable!());
                put(s(c.ty()), "type");
                put(s(c.body()), "value");
            }
            SyntaxKind::TYPE_ALIAS => {
                let t = ast::TypeAlias::cast(node).unwrap_or_else(|| unreachable!());
                put(s(t.ty()), "type");
            }
            SyntaxKind::RECORD_FIELD => {
                let f = ast::RecordField::cast(node).unwrap_or_else(|| unreachable!());
                put(s(f.ty()), "type");
            }
            SyntaxKind::IMPL => {
                let i = ast::Impl::cast(node).unwrap_or_else(|| unreachable!());
                put(s(i.trait_()), "trait");
                put(s(i.self_ty()), "type");
                put(s(i.assoc_item_list()), "body");
            }
            SyntaxKind::TRAIT => {
                let t = ast::Trait::cast(node).unwrap_or_else(|| unreachable!());
                put(s(t.assoc_item_list()), "body");
            }
            SyntaxKind::STRUCT => {
                let st = ast::Struct::cast(node).unwrap_or_else(|| unreachable!());
                put(s(st.field_list()), "body");
            }
            SyntaxKind::UNION => {
                let u = ast::Union::cast(node).unwrap_or_else(|| unreachable!());
                put(s(u.record_field_list()), "body");
            }
            SyntaxKind::ENUM => {
                let e = ast::Enum::cast(node).unwrap_or_else(|| unreachable!());
                put(s(e.variant_list()), "body");
            }
            SyntaxKind::VARIANT => {
                let v = ast::Variant::cast(node).unwrap_or_else(|| unreachable!());
                put(s(v.field_list()), "body");
                put(s(v.const_arg()), "value");
            }
            SyntaxKind::MODULE => {
                let m = ast::Module::cast(node).unwrap_or_else(|| unreachable!());
                put(s(m.item_list()), "body");
            }
            SyntaxKind::EXTERN_BLOCK => {
                let e = ast::ExternBlock::cast(node).unwrap_or_else(|| unreachable!());
                put(s(e.extern_item_list()), "body");
            }
            SyntaxKind::MACRO_CALL => {
                let m = ast::MacroCall::cast(node).unwrap_or_else(|| unreachable!());
                put(s(m.path()), "macro");
            }
            SyntaxKind::USE => {
                let u = ast::Use::cast(node).unwrap_or_else(|| unreachable!());
                put(s(u.use_tree()), "argument");
            }
            SyntaxKind::USE_TREE => {
                let t = ast::UseTree::cast(node).unwrap_or_else(|| unreachable!());
                put(s(t.path()), "path");
                put(s(t.use_tree_list()), "list");
                put(s(t.rename().and_then(|r| r.name())), "alias");
            }
            SyntaxKind::PATH_SEGMENT => {
                let p = ast::PathSegment::cast(node).unwrap_or_else(|| unreachable!());
                put(s(p.generic_arg_list()), "type_arguments");
            }
            _ => {}
        }
    }
    out
}
