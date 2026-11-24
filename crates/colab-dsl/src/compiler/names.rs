//! Names in a script: the metavariables a pattern binds, `scan` phases and
//! the sets their `in` checks read, and `pattern` / `rewriter` items
//! resolved into the rules that name them.

use super::*;

/// The metavariables `pattern` binds, by name (no sigil, `$_` aside).
pub(crate) fn metavariable_names(pattern: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = pattern;
    while let Some(i) = rest.find('$') {
        let after = rest[i..].trim_start_matches('$');
        let len = after
            .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
            .unwrap_or(after.len());
        let name = &after[..len];
        let starts = name.starts_with(|c: char| c.is_ascii_uppercase() || c == '_');
        if !name.is_empty() && name != "_" && starts && !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
        rest = &after[len..];
    }
    out
}

/// A `scan`: its name, the metavariables it collects, and the pattern rule
/// that finds them (`<lang>::pattern` only).
pub(crate) fn lower_scan(
    scan: crate::ast::Scan,
    defs: &includes::Definitions,
    backends: &BackendRegistry,
) -> Result<(String, Vec<String>, Box<dyn Operation>)> {
    let crate::ast::Scan {
        namespace,
        pattern,
        scope,
        constraints,
        name,
        ..
    } = scan;
    if namespace.module != "pattern" {
        return Err(Error::UnsupportedOperation(format!(
            "`scan {}::{}`: a scan collects a pattern's bindings, so it is `{}::pattern '<code>'`",
            namespace.lang, namespace.module, namespace.lang
        )));
    }
    let backend = backends.get(namespace.lang.as_str()).ok_or_else(|| {
        Error::UnsupportedOperation(format!(
            "`scan {}::pattern`: no backend for language `{}`; {}",
            namespace.lang,
            namespace.lang,
            suggest::candidates_note(&namespace.lang, "languages", &backends.languages())
        ))
    })?;
    // Named patterns resolve in a scan's `where` as in a rule's.
    let mut holder = Match {
        namespace: Namespace {
            lang: namespace.lang.clone(),
            module: "pattern".into(),
        },
        match_string: pattern.clone(),
        scope: None,
        fences: None,
        prose: false,
        message: None,
        severity: None,
        id: None,
        url: None,
        constraints,
        lets: Vec::new(),
        action: Action::Delete,
        imports: Vec::new(),
        span: crate::ast::Span { start: 0, end: 0 },
    };
    resolve_names(&mut holder, defs, None)?;
    let spec = RuleSpec::Delete {
        target: pattern.clone(),
    };
    let op = if holder.constraints.is_empty() {
        backend.build_rule("pattern", spec)?
    } else {
        backend.build_rule_where("pattern", spec, &holder.constraints)?
    };
    let op: Box<dyn Operation> = match scope {
        Some(glob) => Box::new(ScopedOperation::new(op, &glob)?),
        None => op,
    };
    Ok((name, metavariable_names(&pattern), op))
}

/// Check each `where $X in <set>` names a scan made before it, and fill in
/// which of the scan's metavariables it means: the one it names
/// (`<set>.$F`), else the one with `$X`'s name, else the scan's only one.
/// `lenient`: an unknown set may come from an include the editor cannot see.
pub(crate) fn resolve_in(
    constraints: &mut [Constraint],
    scans: &std::collections::HashMap<String, Vec<String>>,
    lenient: bool,
) -> Result<()> {
    for c in constraints {
        match &mut c.test {
            colab_core::Test::Any(all) | colab_core::Test::All(all) => {
                resolve_in(all, scans, lenient)?
            }
            colab_core::Test::In { var, set, field } => {
                let Some(vars) = scans.get(set.as_str()) else {
                    if lenient {
                        continue;
                    }
                    let mut known: Vec<&str> = scans.keys().map(String::as_str).collect();
                    known.sort_unstable();
                    let hint = suggest::closest(set, known.iter().copied())
                        .map_or_default(|k| format!(" (did you mean `{k}`?)"));
                    return Err(Error::Config(format!(
                        "`{var} in {set}`: no scan named `{set}` before this rule{hint}; a rule reads the scans of its own stage and earlier ones"
                    )));
                };
                let listed = || {
                    vars.iter()
                        .map(|v| format!("`${v}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                match field {
                    Some(f) if !vars.contains(&f.name) => {
                        return Err(Error::Config(format!(
                            "`{var} in {set}.{f}`: the scan `{set}` binds {}",
                            listed()
                        )));
                    }
                    Some(_) => {}
                    None if vars.contains(&var.name) => {
                        *field = Some(colab_core::Var {
                            name: var.name.clone(),
                            multi: false,
                        });
                    }
                    None if vars.len() == 1 => {
                        *field = Some(colab_core::Var {
                            name: vars[0].clone(),
                            multi: false,
                        });
                    }
                    None => {
                        return Err(Error::Config(format!(
                            "`{var} in {set}`: the scan `{set}` binds {}; say which, e.g. `{var} in {set}.${}`",
                            listed(),
                            vars.first().map_or("X", String::as_str)
                        )));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// `names` in order for a message, with a "did you mean" for `name`.
pub(crate) fn known_names<'a>(
    name: &str,
    names: impl Iterator<Item = &'a String>,
) -> (String, Vec<&'a str>) {
    let mut known: Vec<&str> = names.map(String::as_str).collect();
    known.sort_unstable();
    let hint = suggest::closest(name, known.iter().copied())
        .map_or_default(|k| format!(" (did you mean `{k}`?)"));
    (hint, known)
}

/// Replace each `inside <name>` (and every other named reference) in
/// `m`'s constraints with the named pattern's text, and complete each
/// `rewrite(<name>)` step with its rewriter. With `unknown`, a name `defs`
/// lacks may be defined in an include `check` cannot see: a pattern
/// becomes `unknown`, a rewrite step changes nothing.
pub(crate) fn resolve_names(
    m: &mut Match,
    defs: &includes::Definitions,
    unknown: Option<&str>,
) -> Result<()> {
    for l in &mut m.lets {
        for (step, args) in &mut l.steps {
            if step != "rewrite" {
                continue;
            }
            let names: Option<Vec<String>> = args
                .iter()
                .map(|a| match a {
                    colab_core::TransformArg::Name(name) => Some(name.clone()),
                    _ => None,
                })
                .collect();
            let Some(names) = names else {
                continue;
            };
            let mut expanded = Vec::with_capacity(names.len() * 3);
            for name in &names {
                match defs.rewriters.get(name) {
                    Some((pattern, template)) => {
                        expanded.push(colab_core::TransformArg::Name(name.clone()));
                        expanded.push(colab_core::TransformArg::Text(pattern.clone()));
                        expanded.push(colab_core::TransformArg::Text(template.clone()));
                    }
                    // Perhaps defined in an include `check` cannot see:
                    // the step is checked as one that changes nothing.
                    None if unknown.is_some() => {
                        expanded.clear();
                        break;
                    }
                    None => {
                        let (hint, _) = known_names(name, defs.rewriters.keys());
                        return Err(Error::UnsupportedOperation(format!(
                            "`rewrite({name})`: no rewriter named `{name}`{hint}; define it with \
                             `rewriter {name} = '<pattern>' -> '<template>'`"
                        )));
                    }
                }
            }
            if expanded.is_empty() && !names.is_empty() {
                *step = "strip_prefix".into();
                *args = vec![colab_core::TransformArg::Text(String::new())];
            } else {
                *args = expanded;
            }
        }
    }
    for constraint in &mut m.constraints {
        for pattern in constraint.test.patterns_mut() {
            let Some(name) = colab_core::constraint::pattern_name(pattern) else {
                continue;
            };
            let Some(text) = defs.patterns.get(name).map(String::as_str).or(unknown) else {
                let (hint, known) = known_names(name, defs.patterns.keys());
                let defined = if known.is_empty() {
                    "the script defines none; add `pattern <name> = '<code>'`".to_string()
                } else {
                    format!(
                        "the script defines {}",
                        known
                            .iter()
                            .map(|k| format!("`{k}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                return Err(Error::UnsupportedOperation(format!(
                    "no pattern named `{name}`{hint}; {defined}"
                )));
            };
            *pattern = text.to_string();
        }
    }
    Ok(())
}
