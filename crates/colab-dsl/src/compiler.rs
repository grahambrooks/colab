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

// The compiler's parts: parsing, include expansion, name resolution, and
// lowering a `match` block; this file runs them in order.
mod includes;
mod lower;
/// A match's `let` clauses as the constraints that carry them to a
/// backend, each step checked: what `build_rule_where` receives beside
/// the `where`. Public so a pattern engine can be driven with a rule as
/// the compiler would drive it (`colab-backends/tests/engines.rs`).
pub use lower::lower_lets;
mod names;
mod syntax;

use syntax::in_file;
pub use syntax::parse;

/// Parse and validate `text`, returning an executable [`Refactoring`].
///
/// Returns [`Error::UnsupportedOperation`] when the script targets a
/// namespace or action that no backend in `backends` implements.
///
/// **`include` directives.** This entry point has no base path so
/// `include "..."` raises [`Error::Config`]. Use
/// [`compile_at_path`] for scripts that may include other files.
/// The script's `match` items as the compiler hands them to the backends,
/// before lowering: includes expanded (relative to the working
/// directory), named patterns and rewriters resolved, scan references
/// checked. For driving a pattern engine with a rule exactly as `compile`
/// would (`colab-backends/tests/engines.rs`); `then` stages flattened.
pub fn resolved_matches(text: &str, backends: &BackendRegistry) -> Result<Vec<crate::ast::Match>> {
    let command = parse(text)?;
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut defs = includes::Definitions::default();
    let entries = includes::expand_includes(command.items, None, &mut seen, &mut defs)?;
    let mut scans: std::collections::HashMap<String, Vec<String>> = Default::default();
    let mut out = Vec::new();
    for entry in entries {
        match entry {
            includes::Entry::Match(mut m) => {
                names::resolve_names(&mut m, &defs, None)?;
                names::resolve_in(&mut m.constraints, &scans, false)?;
                out.push(m);
            }
            includes::Entry::Scan(scan) => {
                let (name, vars, _) = names::lower_scan(scan, &defs, backends)?;
                scans.insert(name, vars);
            }
            includes::Entry::Stage => {}
        }
    }
    Ok(out)
}

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
    let mut span = colab_core::telemetry::span("compile");
    let compiled = compile_traced(text, backends, base_path);
    match &compiled {
        Ok(refactoring) => span.set("colab.rules", refactoring.rules.len()),
        Err(e) => span.fail(e.kind()),
    }
    compiled
}

fn compile_traced(
    text: &str,
    backends: &BackendRegistry,
    base_path: Option<&Path>,
) -> Result<Refactoring> {
    let mut command = syntax::parse(text).map_err(|e| syntax::in_file(e, base_path))?;
    // A file of bare rules is named after the file, as reports and
    // `--split-pr` branches show it; `<inline>`, `<stdin>` and MCP's
    // `<script>` are not files, so their rules keep `inline`.
    if crate::inline::is_bare(text)
        && let Some(stem) = base_path
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .filter(|s| !s.starts_with('<'))
    {
        command.refactor_name = stem.to_string();
    }

    let mut seen: HashSet<PathBuf> = HashSet::new();
    if let Some(p) = base_path
        && let Ok(canonical) = p.canonicalize()
    {
        seen.insert(canonical);
    }

    let mut defs = includes::Definitions::default();
    let entries = includes::expand_includes(command.items, base_path, &mut seen, &mut defs)?;
    let mut ids = HashSet::new();
    for id in entries.iter().filter_map(|e| match e {
        includes::Entry::Match(m) => m.id.as_deref(),
        _ => None,
    }) {
        if !ids.insert(id) {
            return Err(Error::Config(format!(
                "two rules have `id \"{id}\"`; an id names one rule"
            )));
        }
    }

    let mut rules: Vec<Box<dyn Operation>> = Vec::new();
    let mut notes = Vec::new();
    let mut stages: Vec<crate::model::Stage> = Vec::new();
    // Scans so far: name → the metavariables it binds.
    let mut scans: std::collections::HashMap<String, Vec<String>> = Default::default();
    for entry in entries {
        let mut m = match entry {
            includes::Entry::Match(m) => m,
            includes::Entry::Stage => {
                // A new stage, unless the current one is still empty.
                let start = stages.last().map_or(0, |s| s.start);
                let has_scans = stages.last().is_some_and(|s| !s.scans.is_empty());
                if rules.len() > start || has_scans {
                    stages.push(crate::model::Stage {
                        start: rules.len(),
                        scans: Vec::new(),
                    });
                }
                continue;
            }
            includes::Entry::Scan(scan) => {
                if scans.contains_key(&scan.name) {
                    return Err(Error::Config(format!(
                        "two scans are named `{}`; give each its own name",
                        scan.name
                    )));
                }
                let (name, vars, op) = names::lower_scan(scan, &defs, backends)?;
                scans.insert(name.clone(), vars);
                match stages.last_mut() {
                    Some(stage) if stage.start == rules.len() => {
                        stage.scans.push(crate::model::ScanOp { name, op })
                    }
                    _ => stages.push(crate::model::Stage {
                        start: rules.len(),
                        scans: vec![crate::model::ScanOp { name, op }],
                    }),
                }
                continue;
            }
        };
        names::resolve_names(&mut m, &defs, None)?;
        names::resolve_in(&mut m.constraints, &scans, false)?;
        let mut note = crate::model::RuleNote {
            message: m.message.clone(),
            severity: m.severity.unwrap_or_default(),
            id: m.id.clone(),
            url: m.url.clone(),
            semantic: m.constraints.iter().any(|c| c.test.is_semantic()),
            script: None,
            advice: Vec::new(),
        };
        let (lowered, advice) = lower::lower_match(m, backends)?;
        note.advice = advice;
        // `and fences` lowers to two rules; both carry the note.
        notes.extend(std::iter::repeat_n(note, lowered.len()));
        rules.extend(lowered);
    }
    Ok(Refactoring {
        name: command.refactor_name,
        rules,
        notes,
        stages,
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
    let command = syntax::parse(text)?;
    // Patterns this file defines. One from an include is unknown here:
    // when the file has includes, an unknown name stands for any node, so
    // the rule is still checked, and [`compile`] reports a name that is
    // really missing.
    let includes = command
        .items
        .iter()
        .any(|item| matches!(item, Item::Include(_)));
    let mut defs = includes::Definitions::default();
    for item in &command.items {
        defs.add(item)?;
    }
    // `then` blocks' rules are checked as the others; scans are known by
    // name, and (in a file with includes) an unknown one may come from one.
    fn flat(items: Vec<Item>, matches: &mut Vec<Match>, scans: &mut Vec<crate::ast::Scan>) {
        for item in items {
            match item {
                Item::Match(m) => matches.push(m),
                Item::Scan(s) => scans.push(s),
                Item::Then(inner) => flat(inner, matches, scans),
                Item::Include(_) | Item::Pattern { .. } | Item::Rewriter { .. } => {}
            }
        }
    }
    let (mut matches, mut scan_items) = (Vec::new(), Vec::new());
    flat(command.items, &mut matches, &mut scan_items);
    let scans: std::collections::HashMap<String, Vec<String>> = scan_items
        .into_iter()
        .map(|s| (s.name.clone(), names::metavariable_names(&s.pattern)))
        .collect();
    Ok(matches
        .into_iter()
        .map(|mut m| {
            let span = m.span;
            let namespace = format!("{}::{}", m.namespace.lang, m.namespace.module);
            let action = m.action.keyword();
            let lowered = names::resolve_names(&mut m, &defs, includes.then_some("$_"))
                .and_then(|()| names::resolve_in(&mut m.constraints, &scans, includes))
                .and_then(|()| lower::lower_match(m, backends));
            let advice = lowered.as_ref().map_or_default(|(_, a)| a.clone());
            let outcome =
                lowered.map(|(ops, _)| ops.iter().map(std::string::ToString::to_string).collect());
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

/// The rules a `match` block lowers to, and its advice.
type Lowered = (Vec<Box<dyn Operation>>, Vec<String>);

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
mod tests;
