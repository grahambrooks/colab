//! `where` constraints and `let` derivations, evaluated as a filter on the
//! match list: relations (`inside`, `has`, siblings), positions, fields,
//! regexes, nested patterns, and the semantic and scan-fact tests.

use super::*;

pub(crate) fn require_bound(bound: &[Var], var: &Var) -> Result<(), String> {
    match bound.iter().find(|b| b.name == var.name) {
        Some(b) if b.multi == var.multi => Ok(()),
        Some(b) => Err(format!(
            "`{var}` is written `{b}` in the pattern; use the same sigil"
        )),
        None if bound.is_empty() => Err(format!(
            "`{var}` is not a metavariable of the pattern, which binds none"
        )),
        None => {
            let names: Vec<String> = bound.iter().map(ToString::to_string).collect();
            Err(format!(
                "`{var}` is not a metavariable of the pattern; it binds {}",
                names.join(", ")
            ))
        }
    }
}

/// The body of every source backend's
/// [`LanguageBackend::build_rule_where`]: a block without constraints is
/// an ordinary [`LanguageBackend::build_rule`]. On `pattern` (with
/// `replace` or `delete`) constraints are compiled by `compile` (the
/// backend's own `PatternOp::compile`); on `symbol` and `call` they may
/// only be `inside` / `not inside`, a syntactic scope ([`crate::scope`]).
pub fn build_rule_where<B, F>(
    backend: &B,
    module: &str,
    spec: RuleSpec,
    constraints: &[Constraint],
    compile: F,
) -> colab_core::Result<Box<dyn Operation>>
where
    B: LanguageBackend + ?Sized,
    F: FnOnce(&str, PatternAction, &[Constraint]) -> colab_core::Result<Box<dyn Operation>>,
{
    if constraints.is_empty() {
        return backend.build_rule(module, spec);
    }
    // `symbol` and `call` take a syntactic scope: `where inside '<pattern>'`.
    if matches!(module, "symbol" | "call") {
        let inner = backend.build_rule(module, spec)?;
        let scoped = crate::scope::Scoped::new(backend.lang(), module, inner, constraints, |p| {
            compile_scope(backend, p)
        })?;
        return Ok(Box::new(scoped));
    }
    if module != "pattern" {
        return Err(colab_core::backend::where_unsupported(
            backend.lang(),
            module,
        ));
    }
    let action = match &spec {
        RuleSpec::Replace { replacement, .. } => PatternAction::Replace(replacement.clone()),
        RuleSpec::Delete { .. } => PatternAction::Delete,
        RuleSpec::Create { path, template, .. } => PatternAction::Create {
            path: path.clone(),
            template: template.clone(),
        },
        _ => return Err(backend.unsupported(module, &spec)),
    };
    compile(spec.target(), action, constraints)
}

/// A scope pattern, compiled as the backend's own `<lang>::pattern` (with
/// `delete`, never applied: only its matches are used).
pub(super) fn compile_scope<B: LanguageBackend + ?Sized>(
    backend: &B,
    pattern: &str,
) -> colab_core::Result<Box<dyn Operation>> {
    backend.build_rule(
        "pattern",
        RuleSpec::Delete {
            target: pattern.to_string(),
        },
    )
}
