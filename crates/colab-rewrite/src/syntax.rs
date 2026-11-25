//! The engine's bridge to `colab_syntax`, colab's own tree and matcher,
//! while the languages still parse with tree-sitter: a grammar's names
//! (built once per grammar), a source text as a colab tree, and a
//! pattern compiled with a backend's [`PatternLanguage`].
//!
//! Nothing in the engine matches through this yet; it is here so the new
//! matcher can be run beside the ast-grep one on the same patterns
//! (`colab-backends/tests/properties.rs`) until it can take over.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use colab_syntax::fragment::{self, Compiled, Fragments};
use colab_syntax::{Grammar, Tree};

use crate::pattern::PatternLanguage;

/// `language`'s kind and field names, built on first use.
pub fn grammar_of(language: &tree_sitter::Language) -> Arc<Grammar> {
    static GRAMMARS: OnceLock<Mutex<HashMap<tree_sitter::Language, Arc<Grammar>>>> =
        OnceLock::new();
    let mut grammars = GRAMMARS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    grammars
        .entry(language.clone())
        .or_insert_with(|| {
            let name = language.name().unwrap_or("?").to_string();
            Arc::new(Grammar::from_tree_sitter(&name, language))
        })
        .clone()
}

/// `source` as a colab tree, parsed through the shared parse cache.
pub fn tree(language: &tree_sitter::Language, source: &str) -> Option<Tree> {
    let parsed = crate::parse_cache::parse(language, source)?;
    Tree::from_tree_sitter(&parsed, source, grammar_of(language)).ok()
}

/// What a form needs from a rewrite beyond its bindings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    /// The pattern as written (or another reading of it).
    Plain,
    /// `to_svg($A)` read as `$COLAB_PATH::to_svg($A)` (or rooted,
    /// `::$COLAB_PATH::to_svg($A)`), so it also finds the function called
    /// by its path; a rewrite puts the path back before the template's
    /// callee.
    Qualified {
        separator: &'static str,
        callee: String,
        rooted: bool,
    },
    /// `S { host: $H }` read with these `(name, VAR)` fields as the
    /// shorthand `S { host }`; a rewrite writes them back the same way.
    Shorthand { fields: Vec<(String, String)> },
}

/// One form of a pattern for the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineForm {
    pub pattern: colab_syntax::pattern::Pattern,
    pub context: Option<String>,
    pub role: Role,
    /// Compiled without the pattern's trailing `;`, which its node does
    /// not hold (a TypeScript class field's): a template's goes too.
    pub loose_semicolon: bool,
}

/// Every form of a pattern, the first the one it reads as first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forms {
    pub forms: Vec<EngineForm>,
    pub tolerant: bool,
}

/// The modifier rules for `language`'s patterns: its modifiers' kinds
/// and its modifier tokens may be left out; a spelled modifiers node is
/// matched exactly.
pub fn rules_for(language: &PatternLanguage) -> colab_syntax::pattern::Rules {
    let grammar = grammar_of(&language.grammar);
    let kinds = |names: &[&str]| -> Vec<colab_syntax::Kind> {
        names.iter().flat_map(|n| grammar.kinds_named(n)).collect()
    };
    let modifiers = language.modifiers.map_or(&[][..], |m| m.kinds);
    let mut absorb = kinds(modifiers);
    absorb.extend(kinds(language.modifier_tokens));
    colab_syntax::pattern::Rules {
        absorb,
        exact: kinds(modifiers),
    }
}

/// `text` compiled for `language` with every form the engine reads it in
/// (see `pattern::compile` for the ast-grep engine's): the readings in
/// each context; the qualified and rooted call; the field shorthand;
/// the pattern without a trailing `;` its node does not hold, or a
/// trailing `,` that changes nothing. Modifiers need no forms: the
/// matcher takes the ones a pattern leaves out ([`rules_for`]).
pub fn compile_forms(language: &PatternLanguage, text: &str) -> Result<Forms, String> {
    let rules = rules_for(language);
    let plain = |compiled: Compiled, loose: bool| -> Vec<EngineForm> {
        compiled
            .forms
            .into_iter()
            .map(|f| EngineForm {
                pattern: f.pattern.with_rules(rules.clone()),
                context: f.context,
                role: Role::Plain,
                loose_semicolon: loose,
            })
            .collect()
    };
    let first = |text: &str| -> Option<colab_syntax::pattern::Pattern> {
        compile_fragment(language, text)
            .ok()
            .and_then(|c| c.forms.into_iter().next())
            .map(|f| f.pattern.with_rules(rules.clone()))
    };
    // A trailing `;` the node does not hold: compiled without it when it
    // reads the same without it, or only so.
    let semicolon = text
        .trim_end()
        .strip_suffix(';')
        .filter(|s| !s.trim().is_empty() && !s.trim_end().ends_with(';'));
    let read = compile_fragment(language, text);
    if let Some(stripped) = semicolon {
        let same = match &read {
            Err(_) => true,
            Ok(with) => compile_fragment(language, stripped)
                .is_ok_and(|w| w.forms[0].pattern == with.forms[0].pattern),
        };
        if same && let Ok(mut loose) = compile_forms(language, stripped) {
            for form in &mut loose.forms {
                form.loose_semicolon = true;
            }
            return Ok(loose);
        }
    }
    let compiled = read?;
    let tolerant = compiled.tolerant;
    let mut forms = plain(compiled, false);
    let add = |form: EngineForm, forms: &mut Vec<EngineForm>| {
        if forms.iter().all(|f| f.pattern != form.pattern) {
            forms.push(form);
        }
    };
    // `name = $V;` is also a class field without its `;`.
    if let Some(stripped) = semicolon
        && let Ok(loose) = compile_fragment(language, stripped)
    {
        let grammar = grammar_of(&language.grammar);
        for form in plain(loose, true) {
            let field = form
                .pattern
                .kind()
                .is_some_and(|k| grammar.kind_name(k).contains("field"));
            if field {
                add(form, &mut forms);
            }
        }
    }
    // A trailing `,` that changes nothing: the candidate's is passed over
    // anyway, and a pattern that spells it would demand it.
    if let Some(stripped) = text.trim_end().strip_suffix(',')
        && let Some(loose) = first(stripped)
        && loose.kind() == forms[0].pattern.kind()
    {
        forms[0].pattern = loose;
    }
    if let Some(sep) = language.qualified_calls
        && let Some((qualified, callee)) = crate::pattern::qualify_call(text, sep)
    {
        let rooted = crate::pattern::root_path(&qualified, sep);
        for (text, rooted) in [(qualified, false), (rooted, true)] {
            if let Some(pattern) = first(&text) {
                let role = Role::Qualified {
                    separator: sep,
                    callee: callee.to_string(),
                    rooted,
                };
                add(
                    EngineForm {
                        pattern,
                        context: None,
                        role,
                        loose_semicolon: false,
                    },
                    &mut forms,
                );
            }
        }
    }
    if let Some(shorthand) = language.shorthand_fields {
        let fields = crate::pattern::shorthand_fields(text, shorthand);
        let subsets: Vec<Vec<usize>> = if fields.len() <= 3 {
            (1..1usize << fields.len())
                .map(|bits| (0..fields.len()).filter(|i| bits & (1 << i) != 0).collect())
                .collect()
        } else {
            vec![(0..fields.len()).collect()]
        };
        for subset in subsets {
            let mut alt = text.to_string();
            for &i in subset.iter().rev() {
                let (range, name, _) = &fields[i];
                alt.replace_range(range.clone(), &format!("{name}{}", shorthand.kept));
            }
            if let Some(pattern) = first(&alt) {
                let fields = subset
                    .iter()
                    .map(|&i| (fields[i].1.clone(), fields[i].2.clone()))
                    .collect();
                add(
                    EngineForm {
                        pattern,
                        context: None,
                        role: Role::Shorthand { fields },
                        loose_semicolon: false,
                    },
                    &mut forms,
                );
            }
        }
    }
    Ok(Forms { forms, tolerant })
}

/// `text` compiled as a pattern for `language` by colab's own matcher:
/// bare or in the language's contexts, with every other reading as a form.
pub fn compile_fragment(language: &PatternLanguage, text: &str) -> Result<Compiled, String> {
    let lang = Fragments {
        expando: language.expando,
        contexts: language.contexts,
        also: language.also,
    };
    // A fragment's parse is never reused, so it skips the shared cache.
    let parse = |filled: &str| {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&language.grammar).ok()?;
        let parsed = parser.parse(filled, None)?;
        Tree::from_tree_sitter(&parsed, filled, grammar_of(&language.grammar)).ok()
    };
    fragment::compile(text, &lang, &parse)
}
