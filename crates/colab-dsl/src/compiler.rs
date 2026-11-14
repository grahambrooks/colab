//! Parse codemod scripts and lower them into the runtime IR.
//!
//! The grammar (see `codemod.lalrpop`) is consumed by [`parse`],
//! which returns a raw [`Command`] AST. [`compile`] /
//! [`compile_at_path`] then expand `include` directives, validate
//! the AST, and ask the supplied [`BackendRegistry`] to produce one
//! [`Operation`] per match block, yielding an executable
//! [`Refactoring`].

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use lalrpop_util::ParseError;
use lalrpop_util::lalrpop_mod;

use crate::ast::{Action, Command, Fences, ImportClause, Item, Let, Match, Namespace};
use crate::model::Refactoring;
use colab_core::suggest;
use colab_core::{
    BackendRegistry, Constraint, Error, FencedOperation, Followup, LanguageBackend, Operation,
    ParseDetail, ProseRename, Result, RuleSpec, ScopedOperation, WithFollowups,
};

lalrpop_mod!(grammar, "/codemod.rs");

/// Parse `text` into a raw AST without applying any semantic checks.
pub fn parse(text: &str) -> Result<Command> {
    grammar::ProgramParser::new().parse(text).map_err(|e| {
        let mut detail = parse_detail(text, e);
        if let Some(hint) = raw_string_hint(text, &detail) {
            detail.message.push_str(&hint);
        }
        Error::Parse(detail)
    })
}

/// When a parse error follows an `r#"` string that closed at an earlier
/// `"#`, the likely cause: the string's text contains `"#` (an anchor in
/// quotes, a raw string shown in documentation). Say where it closed.
fn raw_string_hint(source: &str, detail: &ParseDetail) -> Option<String> {
    let offset = detail.offset.min(source.len());
    // `\"` just before the error: an escape the DSL does not have.
    let line_start = source[..offset].rfind('\n').map_or(0, |i| i + 1);
    if source[line_start..offset].contains("\\\"") {
        return Some(
            "; DSL strings have no escapes, so `\\\"` ends the string. Quote text that holds `\"` \
             with '…', or r#\"…\"# when it holds both quotes"
                .into(),
        );
    }
    let open = source[..offset].rfind("r#\"")?;
    if source[..open].ends_with('#') || source[open..].starts_with("r##") {
        return None;
    }
    let close = open + 3 + source[open + 3..].find("\"#")?;
    if close >= offset {
        return None;
    }
    let line = source[..close].matches('\n').count() + 1;
    let column = close - source[..close].rfind('\n').map_or(0, |i| i + 1) + 1;
    Some(format!(
        "; the raw string opened on line {} closed at the `\"#` on line {line}, column {column}. \
         If its text contains `\"#`, write it as r##\"…\"##",
        source[..open].matches('\n').count() + 1
    ))
}

/// Convert a LALRPOP error into a [`ParseDetail`].
///
/// LALRPOP reports byte offsets and a token list; both are useful, but
/// only after the offset is resolved against the source. Doing that here
/// is what lets the LSP place a diagnostic on the right line and lets an
/// agent fix a script without a second round trip.
fn parse_detail<T: fmt::Display, E: fmt::Display>(
    source: &str,
    error: ParseError<usize, T, E>,
) -> ParseDetail {
    match error {
        ParseError::InvalidToken { location } => {
            ParseDetail::at_offset(source, location, "invalid token", Vec::new())
        }
        ParseError::UnrecognizedEof { location, expected } => {
            ParseDetail::at_offset(source, location, "unexpected end of script", expected)
        }
        ParseError::UnrecognizedToken {
            token: (start, token, _),
            expected,
        } => ParseDetail::at_offset(
            source,
            start,
            format!("unexpected token `{}`", token),
            expected,
        ),
        ParseError::ExtraToken {
            token: (start, token, _),
        } => ParseDetail::at_offset(
            source,
            start,
            format!("unexpected trailing token `{}`", token),
            Vec::new(),
        ),
        ParseError::User { error } => {
            ParseDetail::at_offset(source, 0, error.to_string(), Vec::new())
        }
    }
}

/// Parse and validate `text`, returning an executable [`Refactoring`].
///
/// Returns [`Error::UnsupportedOperation`] when the script targets a
/// namespace or action that no backend in `backends` implements.
///
/// **`include` directives.** This entry point has no base path so
/// `include "..."` raises [`Error::Config`]. Use
/// [`compile_at_path`] for scripts that may include other files.
pub fn compile(text: &str, backends: &BackendRegistry) -> Result<Refactoring> {
    compile_inner(text, backends, None)
}

/// Read `path`, parse it, expand any `include "..."` directives
/// relative to `path`'s parent, and lower the result. The path is
/// canonicalised for cycle detection so back-and-forth includes
/// surface a clear error.
pub fn compile_at_path(path: &Path, backends: &BackendRegistry) -> Result<Refactoring> {
    let text = fs::read_to_string(path).map_err(|e| Error::io_at(path, e))?;
    compile_inner(&text, backends, Some(path))
}

/// Compile script `text` that did not come from a file, resolving
/// `include "..."` relative to `base_path`'s parent.
///
/// This is what lets a caller that receives a script over the wire (the
/// MCP server) support `include` at all: [`compile`] has no base path and
/// must reject includes, and [`compile_at_path`] insists on reading the
/// script from disk.
pub fn compile_at_source(
    text: &str,
    base_path: &Path,
    backends: &BackendRegistry,
) -> Result<Refactoring> {
    compile_inner(text, backends, Some(base_path))
}

fn compile_inner(
    text: &str,
    backends: &BackendRegistry,
    base_path: Option<&Path>,
) -> Result<Refactoring> {
    let command = parse(text)?;

    let mut seen: HashSet<PathBuf> = HashSet::new();
    if let Some(p) = base_path
        && let Ok(canonical) = p.canonicalize()
    {
        seen.insert(canonical);
    }

    let mut defs = Definitions::default();
    let mut matches = expand_includes(command.items, base_path, &mut seen, &mut defs)?;
    for m in &mut matches {
        resolve_names(m, &defs, None)?;
    }

    let mut rules: Vec<Box<dyn Operation>> = Vec::with_capacity(matches.len());
    let mut notes = Vec::with_capacity(matches.len());
    for m in matches {
        let mut note = crate::model::RuleNote {
            message: m.message.clone(),
            severity: m.severity.unwrap_or_default(),
            script: None,
            advice: Vec::new(),
        };
        let (lowered, advice) = lower_match(m, backends)?;
        note.advice = advice;
        // `and fences` lowers to two rules; both carry the note.
        notes.extend(std::iter::repeat_n(note, lowered.len()));
        rules.extend(lowered);
    }
    Ok(Refactoring {
        name: command.refactor_name,
        rules,
        notes,
    })
}

/// One `match` block written in the script itself (not pulled in by an
/// `include`), checked on its own.
#[derive(Debug)]
pub struct RuleCheck {
    /// Where the block sits in the script.
    pub span: crate::ast::Span,
    /// `<lang>::<module>` as written.
    pub namespace: String,
    /// The action keyword as written (`replace`, `delete`, …).
    pub action: &'static str,
    /// The descriptions of the rules the block lowers to (two for `and
    /// fences`, else one), or why it could not be lowered.
    pub outcome: Result<Vec<String>>,
    /// What the rule compiles to but probably should not
    /// ([`Operation::advice`]): editors show these as warnings.
    pub advice: Vec<String>,
}

/// Lower each `match` block of `text` independently, so one bad rule does
/// not hide the next and each error can be placed on its own block.
///
/// A syntax error still fails the whole script — there is no AST to
/// check. `include` directives are not followed here; [`compile`] /
/// [`compile_at_source`] report problems with them.
///
/// This is what the LSP diagnoses with. [`compile`] stops at the first
/// error, which is right for running a script and wrong for editing one.
pub fn check(text: &str, backends: &BackendRegistry) -> Result<Vec<RuleCheck>> {
    let command = parse(text)?;
    // Patterns this file defines. One from an include is unknown here:
    // when the file has includes, an unknown name stands for any node, so
    // the rule is still checked, and [`compile`] reports a name that is
    // really missing.
    let includes = command
        .items
        .iter()
        .any(|item| matches!(item, Item::Include(_)));
    let mut defs = Definitions::default();
    for item in &command.items {
        defs.add(item)?;
    }
    Ok(command
        .items
        .into_iter()
        .filter_map(|item| match item {
            Item::Match(m) => Some(m),
            Item::Include(_) | Item::Pattern { .. } | Item::Rewriter { .. } => None,
        })
        .map(|mut m| {
            let span = m.span;
            let namespace = format!("{}::{}", m.namespace.lang, m.namespace.module);
            let action = m.action.keyword();
            let lowered = resolve_names(&mut m, &defs, includes.then_some("$_"))
                .and_then(|()| lower_match(m, backends));
            let advice = lowered.as_ref().map(|(_, a)| a.clone()).unwrap_or_default();
            let outcome = lowered.map(|(ops, _)| ops.iter().map(|op| op.to_string()).collect());
            RuleCheck {
                span,
                namespace,
                action,
                outcome,
                advice,
            }
        })
        .collect())
}

/// Recursively expand `include` directives, in source order.
/// Returns the flattened list of `Match` clauses ready for lowering.
fn expand_includes(
    items: Vec<Item>,
    base_path: Option<&Path>,
    seen: &mut HashSet<PathBuf>,
    defs: &mut Definitions,
) -> Result<Vec<Match>> {
    let mut out = Vec::new();
    for item in items {
        defs.add(&item)?;
        match item {
            Item::Match(m) => out.push(m),
            Item::Pattern { .. } | Item::Rewriter { .. } => {}
            Item::Include(rel) => {
                let resolved = resolve_include_path(&rel, base_path)?;
                let canonical = resolved
                    .canonicalize()
                    .map_err(|e| Error::io_at(&resolved, e))?;
                if !seen.insert(canonical.clone()) {
                    return Err(Error::Config(format!(
                        "circular include: {} re-includes itself",
                        resolved.display()
                    )));
                }
                let text = fs::read_to_string(&resolved).map_err(|e| Error::io_at(&resolved, e))?;
                let inner = parse(&text)?;
                let nested = expand_includes(inner.items, Some(&resolved), seen, defs)?;
                out.extend(nested);
            }
        }
    }
    Ok(out)
}

/// The script's named patterns and rewriters (its includes' too).
#[derive(Default)]
struct Definitions {
    patterns: std::collections::HashMap<String, String>,
    /// Name → (pattern, template).
    rewriters: std::collections::HashMap<String, (String, String)>,
}

impl Definitions {
    /// Record `item` if it defines something, each name once per kind.
    fn add(&mut self, item: &Item) -> Result<()> {
        let twice = |kind: &str, name: &str| {
            Error::Config(format!(
                "`{kind} {name}` is defined twice; give each {kind} its own name"
            ))
        };
        match item {
            Item::Pattern { name, text, .. } => {
                if self.patterns.insert(name.clone(), text.clone()).is_some() {
                    return Err(twice("pattern", name));
                }
            }
            Item::Rewriter {
                name,
                pattern,
                template,
                ..
            } => {
                let def = (pattern.clone(), template.clone());
                if self.rewriters.insert(name.clone(), def).is_some() {
                    return Err(twice("rewriter", name));
                }
            }
            Item::Match(_) | Item::Include(_) => {}
        }
        Ok(())
    }
}

/// `names` in order for a message, with a "did you mean" for `name`.
fn known_names<'a>(name: &str, names: impl Iterator<Item = &'a String>) -> (String, Vec<&'a str>) {
    let mut known: Vec<&str> = names.map(String::as_str).collect();
    known.sort_unstable();
    let hint = suggest::closest(name, known.iter().copied())
        .map(|k| format!(" (did you mean `{k}`?)"))
        .unwrap_or_default();
    (hint, known)
}

/// Replace each `inside <name>` (and every other named reference) in
/// `m`'s constraints with the named pattern's text, and complete each
/// `rewrite(<name>)` step with its rewriter. With `unknown`, a name `defs`
/// lacks may be defined in an include `check` cannot see: a pattern
/// becomes `unknown`, a rewrite step changes nothing.
fn resolve_names(m: &mut Match, defs: &Definitions, unknown: Option<&str>) -> Result<()> {
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

fn resolve_include_path(relative: &str, base_path: Option<&Path>) -> Result<PathBuf> {
    let p = Path::new(relative);
    if p.is_absolute() {
        return Ok(p.to_path_buf());
    }
    let base = base_path.ok_or_else(|| {
        Error::Config(format!(
            "cannot resolve `include \"{}\"`: no base path. Use `compile_at_path` instead of `compile` to enable includes.",
            relative
        ))
    })?;
    let parent = base.parent().unwrap_or(Path::new("."));
    Ok(parent.join(relative))
}

/// Lower one `match` block. Usually one operation; `and fences` makes two,
/// the rule for its own files and the same rule for Markdown code fences,
/// so the report attributes each. Also returns the block's advice: what
/// it compiles to but probably should not, such as a rewrite that matches
/// its own output.
/// The rules a `match` block lowers to, and its advice.
type Lowered = (Vec<Box<dyn Operation>>, Vec<String>);

fn lower_match(m: Match, backends: &BackendRegistry) -> Result<Lowered> {
    let mut advice = Vec::new();
    let ops = lower_match_ops(m, backends, &mut advice)?;
    Ok((ops, advice))
}

/// `let` clauses as the constraints that carry them to the pattern
/// compiler, each step checked.
fn lower_lets(namespace: &Namespace, action: &Action, lets: Vec<Let>) -> Result<Vec<Constraint>> {
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

fn lower_match_ops(
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
fn with_import_clauses(
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

/// Convenience: count the number of match clauses in `command`,
/// ignoring `include` items. Used by tests.
#[cfg(test)]
fn match_count(command: &Command) -> usize {
    command
        .items
        .iter()
        .filter(|i| matches!(i, Item::Match(_)))
        .count()
}

#[cfg(test)]
mod tests {
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
            parse(r#" refactor "this" { match  go::import "a.b.c" { replace "d.e.f" } } "#)
                .unwrap();

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

        assert_eq!(match_count(&command), 3);
    }

    #[test]
    fn parses_include_directive() {
        let command = parse(
            r#"
            refactor "with-include" {
                include "shared.codemod"
                match go::import "x" { replace "y" }
            }
            "#,
        )
        .unwrap();
        assert_eq!(command.items.len(), 2);
        assert!(matches!(command.items[0], Item::Include(ref s) if s == "shared.codemod"));
        assert!(matches!(command.items[1], Item::Match(_)));
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
        assert_eq!(match_count(&command), 2);
    }

    #[test]
    fn parses_empty_refactor_block() {
        let command = parse(r#"refactor "noop" { }"#).unwrap();
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
        let err = compile(r#"refactor "x" { include "other.codemod" }"#, &registry()).unwrap_err();
        assert!(
            matches!(&err, Error::Config(msg) if msg.contains("compile_at_path")),
            "got: {err}"
        );
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
        let main = dir.join("main.codemod");
        let shared = dir.join("shared.codemod");
        std::fs::write(
            &main,
            r#"refactor "outer" { include "shared.codemod" match go::import "z" { replace "Z" } }"#,
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
        let a = dir.join("a.codemod");
        let b = dir.join("b.codemod");
        std::fs::write(&a, r#"refactor "a" { include "b.codemod" }"#).unwrap();
        std::fs::write(&b, r#"refactor "b" { include "a.codemod" }"#).unwrap();

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
        let command = parse(&format!("refactor \"t\" {{ {rule} }}")).unwrap();
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
        let twice = crate::parse(
            r#"refactor "r" { match go::import "a" in fences and fences { delete } }"#,
        );
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
        assert_eq!(report.rules[0].message.as_deref(), Some("a is gone"));
        assert_eq!(report.rules[0].severity, colab_core::Severity::Warning);
        assert_eq!(report.rules[1].severity, colab_core::Severity::Error);
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
        let warnings = r.report().warnings;
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
        let shown: Vec<String> = both.rules.iter().map(|r| r.to_string()).collect();
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
        let two = "refactor \"r\" { match toml::key \"a\" { set r##\"a raw string ends at \"# here\"## } }";
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
        let shown: Vec<String> = m.constraints.iter().map(|c| c.to_string()).collect();
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
        let shown: Vec<String> = m.constraints.iter().map(|c| c.to_string()).collect();
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
    fn named_patterns_resolve_wherever_a_constraint_takes_one() {
        let script = r#"refactor "t" {
            match go::pattern 'use($X)' where inside loop until closure, not any(follows loop directly) { delete }
            pattern loop = 'for $$$H { $$$B }'
            pattern closure = 'func() { $$$B }'
        }"#;
        let parsed = parse(script).unwrap();
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
        let parsed = parse(script).unwrap();
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
            r#"refactor "t" { include "shared.codemod" match go::pattern 'f()' where inside shared { delete } }"#,
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
                .map(|e| e.to_string())
                .unwrap_or_default()
        };
        let e = err(r#"match go::symbol "a" { let $N = $F | lower replace "b" }"#);
        assert!(e.contains("only `go::pattern` binds metavariables"), "{e}");
        let e = err(r#"match go::pattern '$X.$F()' { let $N = $F | lower delete }"#);
        assert!(e.contains("has no template"), "{e}");
        let e = err(r#"match go::pattern '$X.$F()' { let $N = $F | snak replace '$N' }"#);
        assert!(e.contains("did you mean `snake`"), "{e}");
        let e =
            err(r#"match go::pattern '$X.$F()' { let $N = $F | strip_prefix(1) replace '$N' }"#);
        assert!(e.contains("takes one string"), "{e}");
        let e = err(r#"match go::pattern '$X.$F()' { let $N = $F | lower(1) replace '$N' }"#);
        assert!(e.contains("takes no arguments"), "{e}");
    }

    #[test]
    fn where_and_import_clauses_are_rejected_where_they_cannot_apply() {
        let err = |rule: &str| {
            compile(&format!("refactor \"t\" {{ {rule} }}"), &registry())
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default()
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
}
