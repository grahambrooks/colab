//! Lowering a `match` block to operations: the backend's rule, its `in`
//! scope, fences and prose, `let`s, and import clauses as follow-ups.

use super::*;

/// Lower one `match` block. Usually one operation; `and fences` makes two,
/// the rule for its own files and the same rule for Markdown code fences,
/// so the report attributes each. Also returns the block's advice: what
/// it compiles to but probably should not, such as a rewrite that matches
/// its own output.
pub(crate) fn lower_match(m: Match, backends: &BackendRegistry) -> Result<Lowered> {
    let mut advice = Vec::new();
    let ops = lower_match_ops(m, backends, &mut advice)?;
    Ok((ops, advice))
}

/// `let` clauses as the constraints that carry them to the pattern
/// compiler, each step checked.
pub(crate) fn lower_lets(
    namespace: &Namespace,
    action: &Action,
    lets: Vec<Let>,
) -> Result<Vec<Constraint>> {
    if namespace.module != "pattern" {
        return Err(Error::UnsupportedOperation(format!(
            "`let` derives a value from a metavariable for a `replace` template, and only \
             `{}::pattern` binds metavariables; `{}::{}` has none",
            namespace.lang, namespace.lang, namespace.module
        )));
    }
    if !matches!(action, Action::Replace(_)) {
        return Err(Error::UnsupportedOperation(
            "`let` derives a value for the `replace` template; this rule has no template".into(),
        ));
    }
    lets.into_iter()
        .map(|Let { var, from, steps }| {
            let steps = steps
                .into_iter()
                .map(|(name, args)| colab_core::Transform::parse(&name, args))
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| {
                    Error::UnsupportedOperation(format!("`let {var} = {from} | …`: {e}"))
                })?;
            Ok(Constraint {
                negated: false,
                test: colab_core::Test::Let { var, from, steps },
            })
        })
        .collect()
}

pub(crate) fn lower_match_ops(
    m: Match,
    backends: &BackendRegistry,
    advice: &mut Vec<String>,
) -> Result<Vec<Box<dyn Operation>>> {
    let Match {
        namespace,
        match_string,
        scope,
        fences,
        prose,
        message: _,
        severity: _,
        id: _,
        url: _,
        mut constraints,
        lets,
        action,
        imports,
        span: _,
    } = m;
    let backend = backends.get(namespace.lang.as_str()).ok_or_else(|| {
        Error::UnsupportedOperation(format!(
            "`{}::{}` is not a supported namespace: no backend for language `{}`; {}",
            namespace.lang,
            namespace.module,
            namespace.lang,
            suggest::candidates_note(&namespace.lang, "languages", &backends.languages())
        ))
    })?;
    if !lets.is_empty() {
        constraints.extend(lower_lets(&namespace, &action, lets)?);
    }
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
        Action::Set(value) => RuleSpec::Set {
            target: match_string,
            value,
        },
        Action::Insert(value) => RuleSpec::Insert {
            target: match_string,
            value,
        },
        Action::Create { path, template } => RuleSpec::Create {
            target: match_string,
            path,
            template,
        },
    };
    // A call template that keeps the function's name (`f(ctx, $args)` for
    // `f`) matches its own output, so every run wraps the call again.
    if let RuleSpec::ReplaceCall { target, template } = &spec
        && template.trim_start().starts_with(&format!("{target}("))
    {
        advice.push(format!(
            "the template still calls `{target}`, so every run rewrites its own output again; apply it once (`--write`, then remove the rule), or rename the function in the template"
        ));
    }
    // Text and Markdown write a value as it is: `\n` there is a backslash
    // and an `n`, not a line break (code templates and JSON / TOML values
    // have escapes of their own, so they are left alone).
    if matches!(namespace.lang.as_str(), "text" | "markdown")
        && spec_texts(&spec).iter().any(|t| t.contains("\\n"))
    {
        advice.push(
            "the rule's text holds `\\n`, which colab writes as a backslash and an `n`: DSL strings \
             have no escapes. If a line break was meant, write one inside the quotes"
                .into(),
        );
    }
    // The rule's own advice, from the first operation built (a fenced twin
    // would only repeat it).
    let collected: std::cell::RefCell<Option<Vec<String>>> = std::cell::RefCell::new(None);
    // Import clauses apply to source files only: a code snippet in the
    // docs seldom shows its imports, and adding them would be noise.
    let build = |with_imports: bool| -> Result<Box<dyn Operation>> {
        let operation = if constraints.is_empty() {
            backend.build_rule(namespace.module.as_str(), spec.clone())?
        } else {
            backend.build_rule_where(namespace.module.as_str(), spec.clone(), &constraints)?
        };
        collected
            .borrow_mut()
            .get_or_insert_with(|| operation.advice());
        if with_imports {
            with_import_clauses(operation, &imports, backend)
        } else {
            Ok(operation)
        }
    };
    // A module that edits fences itself (Markdown `text`) takes the clause
    // as one rule, not a fenced twin.
    if let Some(mode) = fences
        && backend.fence_names().is_empty()
        && let Some(operation) =
            backend.build_rule_in_fences(&namespace.module, spec.clone(), mode == Fences::Also)
    {
        let operation = operation?;
        advice.extend(operation.advice());
        return match scope {
            Some(pattern) => Ok(vec![Box::new(ScopedOperation::new(operation, &pattern)?)]),
            None => Ok(vec![operation]),
        };
    }
    let mut operations = Vec::new();
    if fences != Some(Fences::Only) {
        let mut operation = build(true)?;
        if prose {
            let (from, to) = match &spec {
                RuleSpec::Replace {
                    target,
                    replacement,
                } if namespace.module == "symbol" => (target.clone(), replacement.clone()),
                _ => {
                    return Err(Error::UnsupportedOperation(format!(
                        "`and prose` goes with a `symbol` rename (`match {}::symbol \"Old\" and prose {{ replace \"New\" }}`)",
                        namespace.lang
                    )));
                }
            };
            let locate = backend.prose_locator().ok_or_else(|| {
                Error::UnsupportedOperation(format!(
                    "`{}` cannot tell its comments and strings apart, so `and prose` is not supported",
                    namespace.lang
                ))
            })?;
            operation = Box::new(ProseRename::new(operation, &from, &to, locate));
        }
        operations.push(operation);
    }
    if fences.is_some() {
        let names = backend.fence_names();
        if names.is_empty() {
            let hint = if namespace.lang == "markdown" {
                "; of the Markdown modules only `markdown::text` reads code fences"
            } else {
                ""
            };
            return Err(Error::UnsupportedOperation(format!(
                "`{}` rules cannot run in Markdown code fences{hint}",
                namespace.lang
            )));
        }
        let locate = backends
            .get("markdown")
            .and_then(|md| md.fence_locator())
            .ok_or_else(|| {
                Error::UnsupportedOperation(
                    "`in fences` needs the Markdown backend, which is not registered".into(),
                )
            })?;
        operations.push(Box::new(FencedOperation::new(build(false)?, names, locate)));
    }
    advice.extend(collected.into_inner().unwrap_or_default());
    // The glob is validated here, at compile time, so a typo fails the
    // script rather than silently matching nothing at runtime. It narrows
    // fence rules too, by the Markdown file's path.
    match scope {
        Some(pattern) => operations
            .into_iter()
            .map(|op| Ok(Box::new(ScopedOperation::new(op, &pattern)?) as Box<dyn Operation>))
            .collect(),
        None => Ok(operations),
    }
}

/// Wrap `operation` so its `requires import` / `drops import … when
/// unused` clauses run after it, on the files it changed.
/// The texts a rule writes or looks for: its target and its value.
fn spec_texts(spec: &RuleSpec) -> Vec<&str> {
    let mut out = vec![spec.target()];
    match spec {
        RuleSpec::Replace { replacement, .. } => out.push(replacement),
        RuleSpec::Set { value, .. } | RuleSpec::Insert { value, .. } => out.push(value),
        RuleSpec::ReplaceCall { template, .. } => out.push(template),
        RuleSpec::Create { path, template, .. } => out.extend([path.as_str(), template]),
        RuleSpec::Delete { .. } | RuleSpec::Ensure { .. } => {}
    }
    out
}

pub(crate) fn with_import_clauses(
    operation: Box<dyn Operation>,
    imports: &[ImportClause],
    backend: &dyn LanguageBackend,
) -> Result<Box<dyn Operation>> {
    if imports.is_empty() {
        return Ok(operation);
    }
    // Drops run before requires, whatever order they are written in: a
    // file whose only import is being replaced then gets a plain
    // `import "fmt"`, not a group the deletion left with one entry.
    let drops = imports
        .iter()
        .filter(|c| matches!(c, ImportClause::DropsUnused(_)));
    let requires = imports
        .iter()
        .filter(|c| matches!(c, ImportClause::Requires(_)));
    let followups = drops
        .chain(requires)
        .map(|clause| {
            let op = match clause {
                ImportClause::Requires(target) => {
                    let module = backend.import_module().ok_or_else(|| {
                        Error::UnsupportedOperation(format!(
                            "`{}` has no import module, so `{clause}` cannot be used",
                            backend.lang()
                        ))
                    })?;
                    backend.build_rule(
                        module,
                        RuleSpec::Ensure {
                            target: target.clone(),
                        },
                    )?
                }
                ImportClause::DropsUnused(target) => backend.drop_unused_import(target)?,
            };
            Ok(Followup {
                label: clause.to_string(),
                op,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Box::new(WithFollowups::new(operation, followups)))
}
