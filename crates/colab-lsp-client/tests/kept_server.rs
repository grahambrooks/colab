#![cfg(unix)]
//! Kept servers (the default; forced on here with `COLAB_KEEP_SERVER=1`):
//! commands in a row share one server through
//! `colab lsp-daemon`, and each still sees the files as they are on disk.
//! Its own test binary, since it sets the variable for the process.

#![cfg(unix)]

use std::path::Path;
use std::time::Duration;

use colab_lsp_client::{RenameRequest, ServerCommand, rename};

fn fake_server() -> ServerCommand {
    // The example sits in the profile directory's `examples/`, which is
    // `deps/..` with the old target layout and further up with the
    // build-dir layout (`<profile>/build/<pkg>/<hash>/deps/`).
    let exe = std::env::current_exe().unwrap();
    let name = format!("fake_lsp{}", std::env::consts::EXE_SUFFIX);
    let path = exe
        .ancestors()
        .skip(1)
        .map(|dir| dir.join("examples").join(&name))
        .find(|candidate| candidate.exists())
        .unwrap_or_else(|| panic!("fake_lsp example not built near {}", exe.display()));
    ServerCommand {
        program: path.to_string_lossy().into_owned(),
        args: Vec::new(),
    }
}

fn request(root: &Path, offset: usize, to: &str) -> RenameRequest {
    RenameRequest {
        root: root.to_path_buf(),
        file: root.join("a.rs"),
        offset,
        new_name: to.into(),
        timeout: Duration::from_secs(20),
        settle: Duration::from_millis(100),
        file_operations: false,
    }
}

#[test]
fn a_kept_server_serves_commands_in_a_row_from_disk() {
    // SAFETY: this binary has one test, and no thread has been spawned
    // yet (the daemon below starts one), so nothing can read the
    // environment concurrently.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("COLAB_KEEP_SERVER", "1")
    };
    let root = std::env::temp_dir().join(format!("colab-kept-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.rs"), "fn old() {}\nfn main() { old(); }\n").unwrap();
    std::fs::write(root.join("b.rs"), "use crate::old;\n").unwrap();
    let root = root.canonicalize().unwrap();
    let server = fake_server();

    // The daemon this process's commands will find, run in a thread
    // (the test binary cannot be `colab lsp-daemon`).
    let socket = colab_lsp_client::daemon::socket_for(&server.program, &server.args, &root);
    std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
    let _ = std::fs::remove_file(&socket);
    let (program, args, root2, socket2) = (
        server.program.clone(),
        server.args.clone(),
        root.clone(),
        socket.clone(),
    );
    let daemon = std::thread::spawn(move || {
        colab_lsp_client::daemon::serve(&program, &args, &root2, &socket2, Duration::from_secs(60))
    });
    while !socket.exists() {
        std::thread::sleep(Duration::from_millis(10));
    }
    // First command: rename `old`, and write the result as colab would.
    let first = rename(&server, &request(&root, 3, "middle")).unwrap();
    assert_eq!(first.changes.edits.len(), 2);
    for edit in &first.changes.edits {
        std::fs::write(&edit.path, &edit.after).unwrap();
    }
    // Second command, same server: it must see `middle`, from disk, not
    // the `old` the first command had open.
    let second = rename(&server, &request(&root, 3, "newest")).unwrap();
    let a = second
        .changes
        .edits
        .iter()
        .find(|e| e.path.ends_with("a.rs"))
        .expect("a.rs renamed");
    assert_eq!(a.before, "fn middle() {}\nfn main() { middle(); }\n");
    assert_eq!(a.after, "fn newest() {}\nfn main() { newest(); }\n");
    assert_eq!(second.old_name.as_deref(), Some("middle"));

    colab_lsp_client::daemon::stop(&socket).unwrap();
    // `stop` returns once the daemon has let go of its socket, so a command
    // started right after it starts a fresh server, never the dying one.
    assert!(!socket.exists(), "the socket is gone when stop returns");
    daemon.join().unwrap().unwrap();

    // A kept server that dies: the daemon answers the client's waiting
    // request with what the server last wrote to stderr.
    let mut crashing = fake_server();
    crashing.args.push("--crash-on-initialize".into());
    let socket = colab_lsp_client::daemon::socket_for(&crashing.program, &crashing.args, &root);
    let _ = std::fs::remove_file(&socket);
    let (program, args, root2, socket2) = (
        crashing.program.clone(),
        crashing.args.clone(),
        root.clone(),
        socket.clone(),
    );
    let daemon = std::thread::spawn(move || {
        colab_lsp_client::daemon::serve(&program, &args, &root2, &socket2, Duration::from_secs(60))
    });
    while !socket.exists() {
        std::thread::sleep(Duration::from_millis(10));
    }
    let err = rename(&crashing, &request(&root, 3, "x"))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("it said: fake_lsp: indexing...; error: no toolchain for this project"),
        "{err}"
    );
    let _ = daemon.join();
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_dir_all(&root);
}
