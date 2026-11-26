//! Property tests over real code, in every source language.
//!
//! Hand-written cases test what someone thought of; these test what the
//! samples under `tests/samples/<lang>/` happen to hold. For a deterministic
//! spread of named nodes in each sample (every k-th expression, statement or
//! item, capped per file), three invariants:
//!
//! - **self-match**: the node's own text, compiled as `<lang>::pattern`,
//!   finds a match at that node (or one spanning it, give or take a `;`);
//! - **identity**: `replace` with the same text leaves the file byte-identical;
//! - **generalize**: for a call with a number among its arguments, the edit
//!   that changes the number, generalized (`LanguageBackend::generalize`) and
//!   applied to the file, makes that change there and the file still parses.
//!
//! Perfection is not the bar: each check is counted as passed, refused (the
//! pattern did not compile, or the edit did not generalize) or missed, per
//! language, with the node kinds that missed. The counts are blessed into
//! `tests/samples/summary.txt`, and the test fails when they change, so a
//! change in what the engine handles shows up in review. `COLAB_BLESS=1`
//! rewrites the summary. A panic anywhere fails the test outright.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use colab_core::{LanguageBackend, RuleSpec};
use colab_syntax::{Node, Tree};

/// Nodes checked for self-match and identity per sample file, at most.
const PER_FILE: usize = 40;
/// Calls checked for the generalize round trip per sample file, at most.
const CALLS_PER_FILE: usize = 12;
/// The last is self-match again, by colab's own matcher
/// (`colab_syntax::pattern`, through `colab_rewrite::syntax`), compiled
/// with the same contexts, so the two engines are compared node for node
/// until the new one takes over.
const NAMES: [&str; 4] = ["self-match", "identity", "generalize", "matcher"];

mod common;

/// `source` as colab's tree, by the parser its backend reads it with.
fn parse(lang: &str, source: &str) -> Tree {
    common::tree(lang, source)
}

/// Whether `node` holds a syntax error.
fn has_error(node: &Node<'_>) -> bool {
    node.descendants().any(|n| n.is_error() || n.is_missing())
}

/// Counts for one property in one language.
#[derive(Default)]
struct Tally {
    checked: usize,
    passed: usize,
    refused: usize,
    missed: usize,
    /// Node kind → how many missed or were refused, for the summary.
    failing: BTreeMap<String, usize>,
}

impl Tally {
    fn record(&mut self, kind: &str, outcome: Outcome) {
        self.checked += 1;
        match outcome {
            Outcome::Pass => self.passed += 1,
            Outcome::Refused => {
                self.refused += 1;
                *self.failing.entry(format!("{kind} (refused)")).or_default() += 1;
            }
            Outcome::Missed => {
                self.missed += 1;
                *self.failing.entry(format!("{kind} (missed)")).or_default() += 1;
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Outcome {
    Pass,
    Refused,
    Missed,
}

/// Whether a node is a candidate: an expression, statement, item or call
/// of a size a pattern is written at.
fn candidate(lang: &str, node: &Node<'_>, source: &str) -> bool {
    // rust-analyzer's kinds: `*_expr`, `*_stmt`, and items by name.
    const RUST_ITEMS: &[&str] = &[
        "fn", "struct", "enum", "impl", "trait", "module", "const", "static", "use",
    ];
    const KINDS: &[&str] = &[
        "call",
        "invocation",
        "expression",
        "statement",
        "declaration",
        "definition",
        "assignment",
        "item",
        "lambda",
        "closure",
    ];
    let kind = node.kind_name();
    let text = &source[node.range()];
    let shaped = if lang == "rust" {
        kind.ends_with("_expr") || kind.ends_with("_stmt") || RUST_ITEMS.contains(&kind)
    } else {
        KINDS.iter().any(|k| kind.contains(k))
    };
    node.is_named()
        && !node.is_token()
        && !node.is_error()
        && !has_error(node)
        && shaped
        && (3..=300).contains(&text.trim().len())
        && text.lines().count() <= 8
}

/// Every node `keep` accepts, in pre-order, then every k-th of them so at
/// most `cap` remain: deterministic, and spread over the whole file.
fn pick<'t>(tree: &'t Tree, keep: impl Fn(&Node<'t>) -> bool, cap: usize) -> Vec<Node<'t>> {
    let mut all = Vec::new();
    let mut stack = vec![tree.root()];
    while let Some(node) = stack.pop() {
        if keep(&node) {
            all.push(node);
        }
        let children: Vec<Node<'t>> = node.named_children().collect();
        stack.extend(children.into_iter().rev());
    }
    let stride = all.len().div_ceil(cap).max(1);
    all.into_iter().step_by(stride).take(cap).collect()
}

/// Within a couple of bytes at both ends: a `;` or a space either side.
fn near(a: (usize, usize), b: (usize, usize)) -> bool {
    a.0.abs_diff(b.0) <= 2 && a.1.abs_diff(b.1) <= 2
}

fn self_match(backend: &dyn LanguageBackend, source: &str, node: &Node<'_>) -> Outcome {
    let text = &source[node.range()];
    let Ok(op) = backend.build_rule(
        "pattern",
        RuleSpec::Delete {
            target: text.to_string(),
        },
    ) else {
        return Outcome::Refused;
    };
    let range = (node.start(), node.end());
    let found = op.locate(source).unwrap_or_default();
    let hit = found.iter().any(|f| {
        let at = (f.start, f.end);
        near(at, range)
            || (f.start <= range.0 && range.1 <= f.end && f.end - f.start <= range.1 - range.0 + 2)
    });
    if hit { Outcome::Pass } else { Outcome::Missed }
}

/// The backend's pattern setup, for compiling with colab's own matcher.
fn source_language(lang: &str) -> &'static colab_rewrite::SourceLanguage {
    match lang {
        "c" => &colab_lang_c::LANG,
        "cpp" => &colab_lang_cpp::LANG,
        "csharp" => &colab_lang_csharp::LANG,
        "go" => &colab_lang_go::LANG,
        "java" => &colab_lang_java::LANG,
        "js" => &colab_lang_js::LANG,
        "kotlin" => &colab_lang_kotlin::LANG,
        "php" => &colab_lang_php::LANG,
        "python" => &colab_lang_python::LANG,
        "rust" => &colab_lang_rust::LANG,
        "swift" => &colab_lang_swift::LANG,
        "ts" => &colab_lang_ts::LANG,
        "tsx" => &colab_lang_tsx::LANG,
        other => panic!("no backend {other}"),
    }
}

/// Self-match by colab's own matcher: the node's text compiled with the
/// backend's contexts, found in the file at (or around) the node.
fn own_self_match(lang: &str, source: &str, node: &Node<'_>) -> Outcome {
    let language = (source_language(lang).pattern)();
    let text = &source[node.range()];
    let Ok(compiled) = colab_rewrite::syntax::compile_forms(&language, text) else {
        return Outcome::Refused;
    };
    let Some(tree) = colab_rewrite::syntax::source_tree(&language, source) else {
        return Outcome::Missed;
    };
    let range = (node.start(), node.end());
    let hit = compiled.forms.iter().any(|form| {
        form.pattern.find_all(&tree).iter().any(|m| {
            let at = (m.node.start(), m.node.end());
            near(at, range)
                || (at.0 <= range.0 && range.1 <= at.1 && at.1 - at.0 <= range.1 - range.0 + 2)
        })
    });
    if hit { Outcome::Pass } else { Outcome::Missed }
}

fn identity(backend: &dyn LanguageBackend, source: &str, node: &Node<'_>) -> Outcome {
    let text = source[node.range()].to_string();
    let Ok(op) = backend.build_rule(
        "pattern",
        RuleSpec::Replace {
            target: text.clone(),
            replacement: text,
        },
    ) else {
        return Outcome::Refused;
    };
    if op.apply(source) == source {
        Outcome::Pass
    } else {
        Outcome::Missed
    }
}

/// A call's first literal argument (a number, or a string without
/// interpolation), and a different literal of the same sort for it.
fn literal_argument<'t>(lang: &str, call: Node<'t>, source: &str) -> Option<(Node<'t>, String)> {
    let kind = call.kind_name();
    // Swift's `call_suffix` is the argument half of a call, not one.
    if !(kind.contains("call") || kind.contains("invocation")) || kind == "call_suffix" {
        return None;
    }
    // Where `'…'` is a string, not a character.
    let single_quoted = matches!(lang, "js" | "ts" | "tsx" | "python" | "php");
    let mut lists: Vec<Node<'t>> = call
        .named_children()
        .filter(|c| {
            c.kind_name().ends_with("arguments")
                || c.kind_name().ends_with("argument_list")
                || c.kind_name() == "arg_list"
        })
        .collect();
    // Swift: `call_suffix` holds `value_arguments`.
    for suffix in call
        .named_children()
        .filter(|c| c.kind_name() == "call_suffix")
    {
        lists.extend(
            suffix
                .named_children()
                .filter(|c| c.kind_name() == "value_arguments"),
        );
    }
    for list in lists {
        for mut arg in list.named_children() {
            // `argument` / `value_argument` wrappers around one value.
            while arg.named_children().count() == 1 && arg.kind_name().contains("argument") {
                arg = arg.named_children().next()?;
            }
            let text = &source[arg.range()];
            if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
                let value: u64 = text.parse().ok()?;
                return Some((arg, (value + 1).to_string()));
            }
            let quote = text.chars().next()?;
            if text.len() >= 2
                && (quote == '"' || (quote == '\'' && single_quoted))
                && text.ends_with(quote)
                && !text.contains(['\n', '\\', '$', '#', '{'])
            {
                return Some((arg, format!("{}x{quote}", &text[..text.len() - 1])));
            }
        }
    }
    None
}

/// `None` when the node is not a call with a literal argument.
fn generalize(
    lang: &str,
    backend: &dyn LanguageBackend,
    source: &str,
    node: &Node<'_>,
) -> Option<Outcome> {
    let (number, new_value) = literal_argument(lang, *node, source)?;
    let before = &source[node.range()];
    let offset = number.start() - node.start();
    let mut after = before.to_string();
    after.replace_range(offset..offset + number.range().len(), &new_value);
    let Ok(rules) = backend.generalize(before, &after) else {
        return Some(Outcome::Refused);
    };
    let mut result = source.to_string();
    for rule in &rules {
        let Ok(op) = backend.build_rule(
            "pattern",
            RuleSpec::Replace {
                target: rule.pattern.clone(),
                replacement: rule.template.clone(),
            },
        ) else {
            return Some(Outcome::Refused);
        };
        result = op.apply(&result);
    }
    // At that site exactly the number changed (other sites the rule also
    // matches may change too), and the file still parses.
    let parses = !parse(lang, &result).has_error();
    let here = changed_alone(source, &result, number.range(), &new_value);
    Some(if here && parses {
        Outcome::Pass
    } else {
        Outcome::Missed
    })
}

/// Words (identifier and number runs) and single punctuation characters,
/// whitespace dropped, each with its byte offset.
fn tokens(text: &str) -> Vec<(usize, &str)> {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        let mut end = at + c.len_utf8();
        if word(c) {
            while let Some(&(next, d)) = chars.peek() {
                if !word(d) {
                    break;
                }
                end = next + d.len_utf8();
                chars.next();
            }
        }
        out.push((at, &text[at..end]));
    }
    out
}

/// Whether `result`, compared with `source` token by token, has the literal
/// at bytes `site` turned into `new` and nothing around it changed in the
/// same stroke: a rule that also swallowed what surrounds the literal (a
/// wider match rewritten from its template) fails.
fn changed_alone(source: &str, result: &str, site: Range<usize>, new: &str) -> bool {
    use similar::DiffTag;
    let old = tokens(source);
    let old_texts: Vec<&str> = old.iter().map(|(_, t)| *t).collect();
    let new_texts: Vec<&str> = tokens(result).into_iter().map(|(_, t)| t).collect();
    let span: Vec<usize> = (0..old.len())
        .filter(|&i| site.contains(&old[i].0))
        .collect();
    let (Some(&first), Some(&last)) = (span.first(), span.last()) else {
        return false;
    };
    let span = first..last + 1;
    // The tokens the literal became, read through the diff.
    let mut image: Vec<&str> = Vec::new();
    let mut changed = false;
    for op in similar::capture_diff_slices(similar::Algorithm::Myers, &old_texts, &new_texts) {
        let (tag, olds, news) = op.as_tag_tuple();
        if tag == DiffTag::Equal {
            for i in olds.clone().filter(|i| span.contains(i)) {
                image.push(new_texts[news.start + (i - olds.start)]);
            }
            continue;
        }
        let touches = if olds.is_empty() {
            span.start < olds.start && olds.start < span.end
        } else {
            olds.start < span.end && span.start < olds.end
        };
        if !touches {
            continue;
        }
        if olds.start < span.start || span.end < olds.end {
            return false;
        }
        changed = true;
        image.extend(&new_texts[news]);
    }
    let wanted: Vec<&str> = tokens(new).into_iter().map(|(_, t)| t).collect();
    changed && image == wanted
}

fn samples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/samples")
}

#[test]
fn properties_over_the_samples() {
    let registry = colab_backends::registry();
    let mut langs: Vec<PathBuf> = fs::read_dir(samples_dir())
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    langs.sort();
    // `COLAB_PROPERTIES_VERBOSE=1` lists every failing node on stderr.
    let verbose = std::env::var_os("COLAB_PROPERTIES_VERBOSE").is_some();
    let mut problems: Vec<String> = Vec::new();
    // lang → (self-match, identity, generalize)
    let mut tallies: BTreeMap<String, [Tally; 4]> = BTreeMap::new();
    for dir in &langs {
        let lang = dir.file_name().unwrap().to_string_lossy().into_owned();
        let backend = registry
            .get(&lang)
            .unwrap_or_else(|| panic!("no backend {lang}"));
        let mut files: Vec<PathBuf> = fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        files.sort();
        let tally = tallies.entry(lang.clone()).or_default();
        for file in files {
            let source = fs::read_to_string(&file).unwrap();
            let tree = parse(&lang, &source);
            if tree.has_error() {
                problems.push(format!(
                    "{}: the sample does not parse cleanly",
                    file.display()
                ));
                continue;
            }
            // Self-match and identity over the spread of nodes; generalize
            // over the calls with a literal argument.
            let picked = pick(&tree, |n| candidate(&lang, n, &source), PER_FILE);
            let calls = pick(
                &tree,
                |n| literal_argument(&lang, *n, &source).is_some() && candidate(&lang, n, &source),
                CALLS_PER_FILE,
            );
            let jobs = picked
                .into_iter()
                .map(|n| (n, false))
                .chain(calls.into_iter().map(|n| (n, true)));
            for (node, call) in jobs {
                let kind = node.kind_name().to_string();
                let checks = catch_unwind(AssertUnwindSafe(|| {
                    if call {
                        [None, None, generalize(&lang, backend, &source, &node), None]
                    } else {
                        [
                            Some(self_match(backend, &source, &node)),
                            Some(identity(backend, &source, &node)),
                            None,
                            Some(own_self_match(&lang, &source, &node)),
                        ]
                    }
                }));
                match checks {
                    Ok(results) => {
                        for (i, outcome) in results.into_iter().enumerate() {
                            let Some(outcome) = outcome else { continue };
                            if verbose && !matches!(outcome, Outcome::Pass) {
                                eprintln!(
                                    "{lang} {} {} `{kind}` at {}: {:?}",
                                    NAMES[i],
                                    if matches!(outcome, Outcome::Refused) {
                                        "refused"
                                    } else {
                                        "missed"
                                    },
                                    source[..node.start()].matches('\n').count() + 1,
                                    &source[node.range()]
                                );
                            }
                            tally[i].record(&kind, outcome);
                        }
                    }
                    Err(_) => problems.push(format!(
                        "{}: panicked on a `{kind}` at byte {}: {:?}",
                        file.display(),
                        node.start(),
                        &source[node.range()]
                    )),
                }
            }
        }
    }

    let summary = render(&tallies);
    let path = samples_dir().join("summary.txt");
    if std::env::var_os("COLAB_BLESS").is_some() {
        fs::write(&path, &summary).unwrap();
    } else if fs::read_to_string(&path).ok().as_deref() != Some(summary.as_str()) {
        problems.push(format!(
            "tests/samples/summary.txt differs from this run (COLAB_BLESS=1 rewrites it):\n{summary}"
        ));
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

fn render(tallies: &BTreeMap<String, [Tally; 4]>) -> String {
    let mut out = String::from(
        "# Property suite over tests/samples/ (crates/colab-backends/tests/properties.rs).\n\
         # Per language and property: checked / passed / refused / missed.\n\
         # Regenerate with COLAB_BLESS=1 cargo test -p colab-backends --test properties.\n\n",
    );
    let _ = writeln!(
        out,
        "{:<8} {:>22} {:>22} {:>22} {:>22}",
        "lang", NAMES[0], NAMES[1], NAMES[2], NAMES[3]
    );
    for (lang, t) in tallies {
        let cell = |t: &Tally| format!("{}/{}/{}/{}", t.checked, t.passed, t.refused, t.missed);
        let _ = writeln!(
            out,
            "{lang:<8} {:>22} {:>22} {:>22} {:>22}",
            cell(&t[0]),
            cell(&t[1]),
            cell(&t[2]),
            cell(&t[3])
        );
    }
    out.push_str("\n# What failed, by node kind.\n");
    for (lang, t) in tallies {
        for (i, tally) in t.iter().enumerate() {
            if tally.failing.is_empty() {
                continue;
            }
            let kinds: Vec<String> = tally
                .failing
                .iter()
                .map(|(kind, n)| format!("{kind} ×{n}"))
                .collect();
            let _ = writeln!(out, "{lang} {}: {}", NAMES[i], kinds.join(", "));
        }
    }
    out
}
