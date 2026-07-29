//! Walker throughput benchmarks.
//!
//! Builds a synthetic Go corpus of N files, each with the same
//! import shape, then drives the standard go::import rename
//! through the walker at several `--jobs` settings.
//!
//! Run from the repo root:
//!
//! ```sh
//! cargo bench -p colab-cli --bench walker
//! ```
//!
//! The bench exists primarily to catch regressions:
//!
//! - the parallel path should beat the sequential one on multi-core
//!   machines once the corpus is wide enough that walker overhead
//!   dominates;
//! - a multi-rule, mixed-language script should cost roughly what the
//!   single-rule one does, because rules are gated per file
//!   (`is_file_relevant`) and prefiltered on their target literal
//!   before any tree-sitter parse. Without those, cost scales with
//!   rule count regardless of whether a rule can possibly match.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use criterion::{Criterion, criterion_group, criterion_main};

use colab_core::{BackendRegistry, WalkOptions, walker};
use colab_dsl::compile;

const FILE_COUNT: usize = 1_000;
const SCRIPT: &str = r#"refactor "rename" {
    match go::import "old.module" { replace "new.module" }
}"#;

/// Five rules across two languages. Only the first can match a Go file
/// that contains `old.module`; the rest exist to be skipped.
const MIXED_SCRIPT: &str = r#"refactor "mixed" {
    match go::import "old.module" { replace "new.module" }
    match go::symbol "AbsentSymbol" { replace "Renamed" }
    match go::call "absent.Call" { replace_call "present.Call($args)" }
    match rust::use "some_crate" { replace "other_crate" }
    match rust::symbol "AbsentType" { replace "RenamedType" }
}"#;

const FILE_TEMPLATE: &str = r#"package demo

import (
	"fmt"
	"old.module"
)

func run() {
	fmt.Println("hi")
}
"#;

/// A Go file with no import the script targets: the prefilter should
/// keep it from ever being parsed.
const UNMATCHED_TEMPLATE: &str = r#"package demo

import "fmt"

func other() {
	fmt.Println("nothing to rewrite here")
}
"#;

const RUST_TEMPLATE: &str = r#"use std::fmt;

pub fn helper() -> String {
    format!("{}", 1)
}
"#;

fn build_corpus(label: &str, n: usize) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "colab-bench-{}-{}-{}",
        label,
        std::process::id(),
        id
    ));
    fs::create_dir_all(&dir).unwrap();
    for i in 0..n {
        let path = dir.join(format!("dir{:02}/{:04}.go", i % 16, i));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, FILE_TEMPLATE).unwrap();
    }
    dir
}

/// A corpus closer to a real repo: a mix of languages, and only a
/// fraction of the files actually containing the rename target.
///
/// One file in twenty matches. The other nineteen are exactly the case
/// the prefilter exists for — relevant by extension, but impossible to
/// change.
fn build_mixed_corpus(label: &str, n: usize) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "colab-bench-{}-{}-{}",
        label,
        std::process::id(),
        id
    ));
    fs::create_dir_all(&dir).unwrap();
    for i in 0..n {
        let (name, body) = match i % 4 {
            0 if i % 20 == 0 => (format!("dir{:02}/{:04}.go", i % 16, i), FILE_TEMPLATE),
            0..=2 => (
                format!("dir{:02}/{:04}.go", i % 16, i),
                UNMATCHED_TEMPLATE,
            ),
            _ => (format!("dir{:02}/{:04}.rs", i % 16, i), RUST_TEMPLATE),
        };
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, body).unwrap();
    }
    dir
}

fn registry() -> BackendRegistry {
    let mut r = BackendRegistry::new();
    r.register(Box::new(colab_lang_go::GoBackend));
    r.register(Box::new(colab_lang_rust::RustBackend));
    r
}

/// Reproduces the pre-gating execution model: every rule is applied to
/// every relevant file, relying on each one to no-op rather than
/// skipping it. Used to quantify what the gate and the prefilter buy.
struct Ungated<'a, T>(&'a T);

impl<T: colab_core::CodeTransformer> colab_core::CodeTransformer for Ungated<'_, T> {
    fn is_file_relevant(&self, path: &Path) -> bool {
        self.0.is_file_relevant(path)
    }

    fn apply(&self, source_code: &str) -> String {
        self.0.apply(source_code)
    }

    fn apply_at(&self, _path: &Path, source_code: &str) -> colab_core::ApplyOutcome {
        colab_core::ApplyOutcome::untraced(self.0.apply(source_code))
    }
}

fn run_walker(
    refactoring: &(impl colab_core::CodeTransformer + Sync),
    root: &Path,
    jobs: Option<usize>,
) {
    let opts = WalkOptions {
        jobs,
        ..Default::default()
    };
    let mut count = 0u32;
    walker::walk_with(refactoring, root, &opts, &mut |change| {
        if change.changed() {
            count += 1;
        }
        Ok(())
    })
    .unwrap();
    assert!(count > 0);
}

fn bench(c: &mut Criterion) {
    let backends = registry();
    let refactoring = compile(SCRIPT, &backends).unwrap();
    let corpus = build_corpus("corpus", FILE_COUNT);

    let mut group = c.benchmark_group("walker_1k_go_files");
    // Reduce the sample count so the bench finishes in CI.
    group.sample_size(10);

    group.bench_function("sequential (jobs=1)", |b| {
        b.iter(|| run_walker(&refactoring, &corpus, Some(1)));
    });
    group.bench_function("parallel (default)", |b| {
        b.iter(|| run_walker(&refactoring, &corpus, None));
    });
    group.finish();

    fs::remove_dir_all(&corpus).ok();

    // Gating + prefilter: one rule vs five across two languages, over a
    // corpus where only 1 file in 20 contains the rename target. The
    // two should land close together; a large gap means rules are being
    // parsed for files they cannot possibly match.
    let mixed_corpus = build_mixed_corpus("mixed", FILE_COUNT);
    let one_rule = compile(SCRIPT, &backends).unwrap();
    let five_rules = compile(MIXED_SCRIPT, &backends).unwrap();

    let mut group = c.benchmark_group("walker_1k_mixed_files");
    group.sample_size(10);
    group.bench_function("1 rule", |b| {
        b.iter(|| run_walker(&one_rule, &mixed_corpus, None));
    });
    group.bench_function("5 rules, 2 languages", |b| {
        b.iter(|| run_walker(&five_rules, &mixed_corpus, None));
    });
    // The same five rules under the pre-gating model, for scale.
    group.bench_function("5 rules, ungated (baseline)", |b| {
        b.iter(|| run_walker(&Ungated(&five_rules), &mixed_corpus, None));
    });
    group.finish();

    fs::remove_dir_all(&mixed_corpus).ok();
}

criterion_group!(benches, bench);
criterion_main!(benches);
