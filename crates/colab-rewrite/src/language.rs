//! The common floor of every source backend, written once.
//!
//! Each of the twelve source backends offers `<lang>::symbol` (rename),
//! `<lang>::call` (`replace_call`) and `<lang>::pattern`, plus prose
//! ranges, declarations and `colab ast`. They differ only in data — a
//! grammar, the node kinds that count as an identifier, the shape of a
//! call — so a backend declares that data once, as a [`SourceLanguage`],
//! and gets the operations from here:
//!
//! ```ignore
//! pub(crate) static LANG: SourceLanguage = SourceLanguage {
//!     namespace: "c",
//!     extensions: &["c", "h"],
//!     file_names: &[],
//!     pattern: patterns::language,
//!     rename_kinds: &["identifier", "type_identifier", "field_identifier"],
//!     prose_kinds: &["comment", "string_content"],
//!     calls: CallShape::Fields("call_expression"),
//!     qualifiers: None,
//! };
//! ```
//!
//! and its `LanguageBackend::build_rule` keeps only its own modules,
//! ending in `(module, spec) => LANG.build_rule(self, module, spec)`.
//!
//! Before this module the three operations were twelve near-identical
//! copies each, differing in a namespace string and a kinds list: the
//! connascence of algorithm `colab-rewrite` exists to remove.

use std::borrow::Cow;
use std::fmt;
use std::ops::Range;
use std::path::Path;

use colab_core::find::Located;
use colab_core::symbols::Declaration;
use colab_core::{
    ActionCapability, AstInput, Capability, Constraint, Error, LanguageBackend, Operation, Result,
    RuleSpec, SyntaxTree,
};
use tree_sitter::{Node, Tree};

use crate::pattern::{PatternAction, PatternLanguage, PatternRule, Qualifiers};

/// Finds a call the language's own way: given a node, the call's
/// argument list and its callee as a target spells it, or `None` when
/// the node is not a call this backend rewrites.
pub type CallLocator = for<'a> fn(Node<'a>, &'a str) -> Option<(Node<'a>, Cow<'a, str>)>;

/// How `<lang>::call` finds a call and what it replaces.
#[derive(Clone, Copy)]
pub enum CallShape {
    /// One node kind with `function` and `arguments` fields (C, C++, C#,
    /// Go, JS, Python, Rust). The target is the `function` field's text.
    Fields(&'static str),
    /// A callee read by the backend's [`CallLocator`] (Java, Kotlin, PHP,
    /// Ruby, Swift), which keeps a receiver in the target (`Old.run`).
    Located {
        locate: CallLocator,
        /// Replace up to the end of the argument list rather than the
        /// whole call node, so a trailing lambda or closure after the
        /// parentheses survives (Kotlin, Swift).
        through_arguments: bool,
    },
}

/// Everything a source backend tells the common floor about its language.
pub struct SourceLanguage {
    /// The DSL namespace: `c`, `go`, `rust`, …
    pub namespace: &'static str,
    /// The file extensions the floor's operations run on.
    pub extensions: &'static [&'static str],
    /// Whole file names they also run on (Ruby's `Rakefile`, …).
    pub file_names: &'static [&'static str],
    /// The `<lang>::pattern` grammar, expando and contexts.
    pub pattern: fn() -> PatternLanguage,
    /// The node kinds `<lang>::symbol` renames.
    pub rename_kinds: &'static [&'static str],
    /// The node kinds that are prose: comments and string contents.
    pub prose_kinds: &'static [&'static str],
    /// How `<lang>::call` finds a call.
    pub calls: CallShape,
    /// The qualifier check for pattern templates (Go, Python, JS), whose
    /// imports bind the qualifier; `None` stays silent.
    pub qualifiers: Option<Qualifiers>,
}

impl fmt::Debug for SourceLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SourceLanguage")
            .field("namespace", &self.namespace)
            .finish_non_exhaustive()
    }
}

/// The actions every `<lang>::pattern` takes.
pub const PATTERN_ACTIONS: &[ActionCapability] = &[
    ActionCapability {
        name: "replace",
        description: "Replace each match with the template, substituting `$NAME` / `$$$NAME` bindings.",
    },
    ActionCapability {
        name: "delete",
        description: "Remove each match; a match alone on its line takes the line with it.",
    },
    ActionCapability {
        name: "create",
        description: "`create \"<path>\" '<template>'`: a new file from each match, at the path (relative to the matched file's directory) with the template's text, both filled from the bindings. The matched file is left as it is; a file already at the path with other text is a conflict.",
    },
];

/// The `pattern` row of a backend's capabilities; `description` names
/// the language and gives an example.
pub const fn pattern_capability(description: &'static str) -> Capability {
    Capability {
        module: "pattern",
        description,
        actions: PATTERN_ACTIONS,
    }
}

impl SourceLanguage {
    /// Whether the floor's operations run on `path`.
    pub fn is_source(&self, path: &Path) -> bool {
        if let Some(ext) = path.extension().and_then(|s| s.to_str())
            && self.extensions.contains(&ext)
        {
            return true;
        }
        !self.file_names.is_empty()
            && path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|name| self.file_names.contains(&name))
    }

    /// `source`'s syntax tree, through the shared parse cache: a text the
    /// rules before have already parsed is not parsed again. `None` if
    /// tree-sitter cannot parse.
    pub fn parse(&self, source: &str) -> Option<Tree> {
        crate::parse_cache::parse(&(self.pattern)().grammar, source)
    }

    /// `source` with every identifier `from` renamed to `to`
    /// (`<lang>::symbol`).
    pub fn rename(&self, from: &str, to: &str, source: &str) -> String {
        match self.parse(source) {
            Some(tree) => crate::rename_nodes_by_text(&tree, source, self.rename_kinds, from, to),
            None => source.to_string(),
        }
    }

    /// `source` with every call to `function` replaced by `template`
    /// (`<lang>::call`).
    pub fn rewrite_calls(&self, function: &str, template: &str, source: &str) -> String {
        let Some(tree) = self.parse(source) else {
            return source.to_string();
        };
        match self.calls {
            CallShape::Fields(kind) => {
                crate::calls::rewrite(&tree, source, kind, function, template)
            }
            CallShape::Located {
                locate,
                through_arguments,
            } => crate::calls::rewrite_located(
                &tree,
                source,
                locate,
                through_arguments,
                function,
                template,
            ),
        }
    }

    /// Every callee in `source`, as a `<lang>::call` target would spell it.
    pub fn callees(&self, source: &str) -> Vec<String> {
        let Some(tree) = self.parse(source) else {
            return Vec::new();
        };
        match self.calls {
            CallShape::Fields(kind) => {
                crate::field_texts(&tree, source, &[kind], Some("function"), &[])
            }
            CallShape::Located { locate, .. } => {
                crate::calls::callees_located(&tree, source, locate)
            }
        }
    }

    /// A conflict for each function the change from `before` to `after`
    /// made call itself more often than it did: a rewrite to a new helper
    /// that also rewrote the call inside that helper (`unified_diff(…)` →
    /// `change_diff(…)` inside `fn change_diff`) recurses for ever. Judged
    /// by name, functions and methods only, so it is silent when unsure.
    pub fn new_self_calls(&self, before: &str, after: &str) -> Vec<String> {
        let was = self.self_calls(before);
        self.self_calls(after)
            .into_iter()
            .filter(|(name, n)| *n > was.get(name).copied().unwrap_or(0))
            .map(|(name, _)| {
                format!(
                    "the rewrite makes `{name}` call itself, which it did not before: a match \
                     inside `{name}` became a call to it. Narrow the rule with `where not inside` \
                     that function, or allow the conflict if the recursion is meant"
                )
            })
            .collect()
    }

    /// How many calls each function or method in `source` makes to its own
    /// name (the callee's last path segment), by name.
    fn self_calls(&self, source: &str) -> std::collections::BTreeMap<String, usize> {
        fn walk(
            lang: &SourceLanguage,
            source: &str,
            decls: &[colab_core::symbols::Declaration],
            out: &mut std::collections::BTreeMap<String, usize>,
        ) {
            for decl in decls {
                if decl.kind.contains("function") || decl.kind.contains("method") {
                    let body = &source[decl.range.clone()];
                    let own = lang
                        .callees(body)
                        .iter()
                        .filter(|callee| is_self_call(callee, &decl.name))
                        .count();
                    *out.entry(decl.name.clone()).or_default() += own;
                }
                walk(lang, source, &decl.children, out);
            }
        }
        let mut out = std::collections::BTreeMap::new();
        walk(self, source, &self.declarations(source), &mut out);
        out
    }

    /// For `LanguageBackend::generalize` (`colab.generalize`).
    pub fn generalize(&self, before: &str, after: &str) -> Result<colab_core::Generalized> {
        crate::generalize::generalize(&(self.pattern)(), before, after)
            .map_err(|e| Error::Config(format!("{}: {e}", self.namespace)))
    }

    /// For `LanguageBackend::prose_ranges`.
    pub fn prose_ranges(&self, source: &str) -> Option<Vec<Range<usize>>> {
        self.parse(source)
            .map(|tree| crate::ranges_of_kinds(&tree, self.prose_kinds))
    }

    /// For `LanguageBackend::declarations`.
    pub fn declarations(&self, source: &str) -> Vec<Declaration> {
        crate::declarations(&(self.pattern)().grammar, source)
    }

    /// For `LanguageBackend::syntax_tree` (`colab ast`).
    pub fn syntax_tree(&self, input: AstInput<'_>) -> Result<SyntaxTree> {
        let (text, is_pattern) = match input {
            AstInput::Source(text) => (text, false),
            AstInput::Pattern(text) => (text, true),
        };
        let report = crate::pattern::describe(&(self.pattern)(), text, is_pattern)
            .map_err(|e| self.pattern_error(&e))?;
        Ok(SyntaxTree {
            tree: report.tree,
            has_error: report.has_error,
            context: report.context,
            prefilter: report.prefilter,
        })
    }

    fn pattern_error(&self, e: &str) -> Error {
        Error::UnsupportedOperation(format!("{}::pattern: {e}", self.namespace))
    }

    /// A `<lang>::symbol "from" { replace "to" }` rule.
    pub fn symbol_rename(&'static self, from: &str, to: &str) -> SymbolRename {
        SymbolRename {
            lang: self,
            from: from.to_string(),
            to: to.to_string(),
        }
    }

    /// A `<lang>::call "function" { replace_call "template" }` rule.
    pub fn call_replace(&'static self, function: &str, template: &str) -> CallReplace {
        CallReplace {
            lang: self,
            function: function.to_string(),
            template: template.to_string(),
        }
    }

    /// A `<lang>::pattern` rule.
    pub fn compile_pattern(
        &'static self,
        pattern: &str,
        action: PatternAction,
        constraints: &[Constraint],
    ) -> Result<PatternOp> {
        let rule = PatternRule::compile(&(self.pattern)(), pattern, action, constraints)
            .map_err(|e| self.pattern_error(&e))?;
        let rule = match self.qualifiers {
            Some(qualifiers) => rule.with_qualifiers(qualifiers),
            None => rule,
        };
        Ok(PatternOp { lang: self, rule })
    }

    /// The floor's half of `LanguageBackend::build_rule`: `symbol`,
    /// `call` and `pattern`, and the standard rejection of anything else.
    /// A backend matches its own modules first and hands the rest here.
    pub fn build_rule<B: LanguageBackend + ?Sized>(
        &'static self,
        backend: &B,
        module: &str,
        spec: RuleSpec,
    ) -> Result<Box<dyn Operation>> {
        match (module, spec) {
            (
                "symbol",
                RuleSpec::Replace {
                    target,
                    replacement,
                },
            ) => Ok(Box::new(self.symbol_rename(&target, &replacement))),
            ("call", RuleSpec::ReplaceCall { target, template }) => {
                Ok(Box::new(self.call_replace(&target, &template)))
            }
            (
                "pattern",
                RuleSpec::Replace {
                    target,
                    replacement,
                },
            ) => Ok(Box::new(self.compile_pattern(
                &target,
                PatternAction::Replace(replacement),
                &[],
            )?)),
            ("pattern", RuleSpec::Delete { target }) => Ok(Box::new(self.compile_pattern(
                &target,
                PatternAction::Delete,
                &[],
            )?)),
            (
                "pattern",
                RuleSpec::Create {
                    target,
                    path,
                    template,
                },
            ) => Ok(Box::new(self.compile_pattern(
                &target,
                PatternAction::Create { path, template },
                &[],
            )?)),
            (other, spec) => Err(backend.unsupported(other, &spec)),
        }
    }

    /// `LanguageBackend::build_rule_where` for a source backend: patterns
    /// take every constraint, `symbol` and `call` a syntactic scope
    /// ([`crate::pattern::build_rule_where`]).
    pub fn build_rule_where<B: LanguageBackend + ?Sized>(
        &'static self,
        backend: &B,
        module: &str,
        spec: RuleSpec,
        constraints: &[Constraint],
    ) -> Result<Box<dyn Operation>> {
        crate::pattern::build_rule_where(backend, module, spec, constraints, |p, a, c| {
            Ok(Box::new(self.compile_pattern(p, a, c)?))
        })
    }
}

/// `<lang>::symbol`: rename an identifier and its in-file usages.
///
/// Syntactic: every node of [`SourceLanguage::rename_kinds`] whose text
/// equals `from`, in each file processed. No scope analysis.
#[derive(Debug)]
pub struct SymbolRename {
    lang: &'static SourceLanguage,
    pub from: String,
    pub to: String,
}

impl fmt::Display for SymbolRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}::symbol \"{}\" -> \"{}\"",
            self.lang.namespace, self.from, self.to
        )
    }
}

impl Operation for SymbolRename {
    fn is_file_relevant(&self, path: &Path) -> bool {
        self.lang.is_source(path)
    }

    fn apply(&self, source_code: &str) -> String {
        self.lang.rename(&self.from, &self.to, source_code)
    }

    fn prefilter(&self) -> Option<&str> {
        Some(&self.from)
    }

    fn near_miss_target(&self) -> Option<&str> {
        Some(&self.from)
    }

    fn observed_targets(&self, source: &str) -> Vec<String> {
        self.lang.parse(source).map_or_default(|tree| {
            crate::field_texts(&tree, source, self.lang.rename_kinds, None, &[])
        })
    }

    fn conflicts(&self, before: &str, _after: &str) -> Vec<String> {
        self.lang
            .parse(before)
            .and_then(|tree| {
                crate::rename_conflict(&tree, before, self.lang.rename_kinds, &self.from, &self.to)
            })
            .into_iter()
            .collect()
    }
}

/// `<lang>::call`: rewrite each call to `function` with `template`.
///
/// Matching is exact equality on the callee text, so `old_fn` and
/// `ns_old_fn` are distinct. A template that keeps the function name
/// matches its own output and wraps again on the next pass: templates
/// must rename the function to be re-runnable.
#[derive(Debug)]
pub struct CallReplace {
    lang: &'static SourceLanguage,
    pub function: String,
    pub template: String,
}

impl fmt::Display for CallReplace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}::call \"{}\" -> replace_call \"{}\"",
            self.lang.namespace, self.function, self.template
        )
    }
}

impl Operation for CallReplace {
    fn is_file_relevant(&self, path: &Path) -> bool {
        self.lang.is_source(path)
    }

    fn apply(&self, source_code: &str) -> String {
        self.lang
            .rewrite_calls(&self.function, &self.template, source_code)
    }

    fn prefilter(&self) -> Option<&str> {
        Some(&self.function)
    }

    fn near_miss_target(&self) -> Option<&str> {
        Some(&self.function)
    }

    fn observed_targets(&self, source: &str) -> Vec<String> {
        self.lang.callees(source)
    }

    fn conflicts(&self, before: &str, after: &str) -> Vec<String> {
        self.lang.new_self_calls(before, after)
    }
}

/// `<lang>::pattern`: a [`PatternRule`] on the language's files.
#[derive(Debug)]
pub struct PatternOp {
    lang: &'static SourceLanguage,
    rule: PatternRule,
}

impl PatternOp {
    /// The compiled rule.
    pub fn rule(&self) -> &PatternRule {
        &self.rule
    }
}

impl fmt::Display for PatternOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}::{}", self.lang.namespace, self.rule)
    }
}

impl Operation for PatternOp {
    fn is_file_relevant(&self, path: &Path) -> bool {
        self.lang.is_source(path)
    }

    fn apply(&self, source_code: &str) -> String {
        self.rule.apply(source_code)
    }

    fn prefilter(&self) -> Option<&str> {
        self.rule.prefilter()
    }

    fn locate(&self, source: &str) -> Option<Vec<Located>> {
        Some(
            self.rule
                .find(source)
                .into_iter()
                .map(|f| Located::at(source, f.start, f.end, f.captures))
                .collect(),
        )
    }

    fn conflicts(&self, before: &str, after: &str) -> Vec<String> {
        let mut found = self.rule.conflicts(before, after);
        found.extend(self.lang.new_self_calls(before, after));
        found
    }

    fn creates(&self, path: &Path, source: &str) -> Vec<colab_core::Creation> {
        let dir = path.parent().unwrap_or(Path::new(""));
        self.rule
            .creates(source)
            .into_iter()
            .map(|(relative, text, line)| colab_core::Creation {
                path: normalize(&dir.join(relative)),
                text,
                line,
            })
            .collect()
    }

    fn why_unchanged(&self, source: &str) -> Option<String> {
        self.rule.why_unchanged(source)
    }

    fn advice(&self) -> Vec<String> {
        self.rule.advice()
    }
}

/// Whether `callee` calls the function or method named `name` itself: bare
/// (`name`), or through the item's own receiver or type (`self.name`,
/// `Self::name`, `this.name`, `$this->name`). Another value's method of the
/// same name (`LANG.name`, `inner.name`) is a delegation, not recursion.
fn is_self_call(callee: &str, name: &str) -> bool {
    let callee: String = callee.chars().filter(|c| !c.is_whitespace()).collect();
    let Some(qualifier) = callee.strip_suffix(name) else {
        return false;
    };
    matches!(
        qualifier,
        "" | "self." | "Self::" | "self::" | "this." | "$this->" | "self->" | "static::"
    )
}

/// `path` with `.` dropped and each `..` taking the segment before it
/// back, so `./src/../lib/x.rs` reads `lib/x.rs`; a leading `..` stays.
fn normalize(path: &Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir
                if matches!(out.components().next_back(), Some(Component::Normal(_))) =>
            {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}
