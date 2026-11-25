//! The LSP commands against a real language server, not the fake one.
//!
//! Every other LSP test drives `examples/fake_lsp.rs`, which answers as the
//! tests expect a server to; these check that rust-analyzer still does.
//! They are ignored by default, since they need the server and take
//! seconds; CI runs them weekly (`cargo test -p colab-cli --test real_lsp
//! -- --ignored`). Run that way without rust-analyzer installed, they fail
//! rather than pass with nothing checked.

mod common;
use common::*;

const LIB: &str = "mod util;\n\n/// Calls old_name.\npub fn run() -> u32 {\n    util::old_name(2) + util::old_name(3)\n}\n";
const UTIL: &str = "pub fn old_name(x: u32) -> u32 {\n    x\n}\n";

/// A one-crate Cargo project with no dependencies, so rust-analyzer needs
/// no network.
fn crate_workspace(label: &str) -> PathBuf {
    let root = workspace_temp(label);
    write(
        &root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    );
    write(&root.join("src/lib.rs"), LIB);
    write(&root.join("src/util.rs"), UTIL);
    root
}

fn run(root: &std::path::Path, args: &[&str]) -> std::process::Output {
    let output = colab().args(args).arg("-C").arg(root).output().unwrap();
    assert!(
        output.status.success(),
        "colab {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
#[ignore = "needs rust-analyzer; CI runs it weekly"]
fn rust_analyzer_renames_a_function_across_files() {
    let root = crate_workspace("real-rename");
    run(
        &root,
        &[
            "rename",
            "--symbol",
            "old_name",
            "--in",
            "src/util.rs",
            "--to",
            "new_name",
            "--write",
        ],
    );
    assert_eq!(
        fs::read_to_string(root.join("src/util.rs")).unwrap(),
        UTIL.replace("old_name", "new_name")
    );
    // The calls are renamed; the doc comment is prose, which a scope-aware
    // rename leaves alone (`--include-text` would take it).
    assert_eq!(
        fs::read_to_string(root.join("src/lib.rs")).unwrap(),
        LIB.replace("util::old_name", "util::new_name")
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
#[ignore = "needs rust-analyzer; CI runs it weekly"]
fn rust_analyzer_finds_the_references_in_another_file() {
    let root = crate_workspace("real-references");
    let output = run(
        &root,
        &[
            "references",
            "--symbol",
            "old_name",
            "--in",
            "src/util.rs",
            "--format",
            "json",
        ],
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert_eq!(text.matches("src/lib.rs").count(), 2, "{text}");
    fs::remove_dir_all(&root).ok();
}
