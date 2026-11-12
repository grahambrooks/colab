//! Drive the client against `examples/fake_lsp.rs`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use colab_lsp_client::{RenameRequest, ServerCommand, rename};

/// `cargo test` builds this package's examples next to the test binary's
/// `deps/` directory.
fn fake_server() -> ServerCommand {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap().parent().unwrap().join("examples");
    let path = dir.join(format!("fake_lsp{}", std::env::consts::EXE_SUFFIX));
    assert!(
        path.exists(),
        "fake_lsp example not built at {}",
        path.display()
    );
    ServerCommand {
        program: path.to_string_lossy().into_owned(),
        args: Vec::new(),
    }
}

fn workspace(label: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("colab-lsp-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, text) in files {
        std::fs::write(dir.join(name), text).unwrap();
    }
    dir.canonicalize().unwrap()
}

fn request(root: &Path, file: &str, offset: usize, new_name: &str) -> RenameRequest {
    RenameRequest {
        root: root.to_path_buf(),
        file: root.join(file),
        offset,
        new_name: new_name.into(),
        timeout: Duration::from_secs(20),
        settle: Duration::from_millis(100),
    }
}

/// Run with `FAKE_LSP_MODE` set for just this child: the variable is read
/// by the server at start-up, so tests set it on the command, never on
/// this process.
fn server_in_mode(mode: &str) -> ServerCommand {
    // The fake reads the mode from its environment; wrap via `env`.
    let fake = fake_server();
    ServerCommand {
        program: "env".into(),
        args: vec![format!("FAKE_LSP_MODE={mode}"), fake.program],
    }
}

#[test]
fn renames_across_files_after_indexing_and_a_retry() {
    let root = workspace(
        "rename",
        &[
            ("a.rs", "fn old() {}\nfn main() { old(); }\n"),
            ("b.rs", "use crate::old;\n"),
            ("c.txt", "old\n"),
        ],
    );
    let text = std::fs::read_to_string(root.join("a.rs")).unwrap();
    let outcome = rename(
        &fake_server(),
        &request(&root, "a.rs", text.find("old").unwrap() + 1, "new"),
    )
    .unwrap();

    assert_eq!(outcome.old_name.as_deref(), Some("old"));
    let names: Vec<_> = outcome
        .changes
        .iter()
        .map(|c| c.path.file_name().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["a.rs", "b.rs"]);
    assert_eq!(
        outcome.changes[0].after,
        "fn new() {}\nfn main() { new(); }\n"
    );
    assert_eq!(outcome.changes[1].after, "use crate::new;\n");
    assert_eq!(
        std::fs::read_to_string(root.join("a.rs")).unwrap(),
        text,
        "the client never writes"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn nothing_renameable_at_the_position_is_an_error() {
    let root = workspace("nothing", &[("a.rs", "fn old() {}\n")]);
    // `{` — punctuation, not a symbol.
    let at = "fn old() {}\n".find('{').unwrap();
    let err = rename(&fake_server(), &request(&root, "a.rs", at, "x")).unwrap_err();
    assert!(err.to_string().contains("nothing to rename"), "{err}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_server_without_rename_is_refused() {
    let root = workspace("norename", &[("a.rs", "fn old() {}\n")]);
    let err = rename(
        &server_in_mode("no-rename"),
        &request(&root, "a.rs", 4, "x"),
    )
    .unwrap_err();
    assert!(err.to_string().contains("does not support rename"), "{err}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn file_operations_in_the_answer_are_refused() {
    let root = workspace("fileop", &[("a.rs", "fn old() {}\n")]);
    let err = rename(&server_in_mode("file-op"), &request(&root, "a.rs", 4, "x")).unwrap_err();
    assert!(err.to_string().contains("would rename a file"), "{err}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_server_still_indexing_at_the_timeout_is_not_trusted() {
    let root = workspace("slow", &[("a.rs", "fn old() {}\n")]);
    let mut req = request(&root, "a.rs", 4, "x");
    req.timeout = Duration::from_secs(1);
    let err = rename(&server_in_mode("slow"), &req).unwrap_err();
    assert!(err.to_string().contains("still indexing"), "{err}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_missing_server_says_how_to_fix_it() {
    let root = workspace("missing", &[("a.rs", "fn old() {}\n")]);
    let server = ServerCommand::parse("definitely-not-a-language-server-xyz").unwrap();
    let err = rename(&server, &request(&root, "a.rs", 4, "x")).unwrap_err();
    assert!(err.to_string().contains("--lsp"), "{err}");
    std::fs::remove_dir_all(&root).ok();
}
