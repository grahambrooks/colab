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
//! The bench exists primarily to catch regressions: the parallel
//! path should beat the sequential one on multi-core machines once
//! the corpus is wide enough that walker overhead dominates.

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
const FILE_TEMPLATE: &str = r#"package demo

import (
	"fmt"
	"old.module"
)

func run() {
	fmt.Println("hi")
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

fn registry() -> BackendRegistry {
    let mut r = BackendRegistry::new();
    r.register(Box::new(colab_lang_go::GoBackend));
    r
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
}

criterion_group!(benches, bench);
criterion_main!(benches);
