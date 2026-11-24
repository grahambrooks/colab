//! `colab mv` and `colab.mv`: plan a file move with git, and the
//! language server's reference updates (`workspace/willRenameFiles`).

use super::*;

/// What `colab mv` and `colab.mv` move, and how.
pub(crate) struct MoveSpec<'a> {
    pub cwd: &'a Path,
    pub from: &'a Path,
    pub to: &'a Path,
    pub lsp: Option<&'a str>,
    pub no_lsp: bool,
    pub root: Option<&'a Path>,
    pub timeout: u64,
}

/// A move, worked out but not made: the file operation plus the edits the
/// server asked for to keep references pointing at it.
pub(crate) struct MovePlan {
    pub changes: ChangeSet,
    pub label: String,
    pub warnings: Vec<String>,
    pub server: Option<ServerCommand>,
    pub root: PathBuf,
}

/// Plan `spec.from` → `spec.to` (into it, when `to` is a directory): the
/// server's `workspace/willRenameFiles` edits, or the move alone when
/// there is no server.
pub(crate) fn plan_move(spec: &MoveSpec<'_>) -> Result<MovePlan> {
    use colab_core::fileops::FileOp;
    use colab_lsp_client::MoveRequest;
    let from =
        fs::canonicalize(spec.cwd.join(spec.from)).map_err(|e| Error::io_at(spec.from, e))?;
    let mut to = spec.cwd.join(spec.to);
    if to.is_dir() {
        let name = from
            .file_name()
            .ok_or_else(|| Error::Config(format!("cannot move {}", from.display())))?;
        to = to.join(name);
    }
    // Resolve `to` through its parent, so it compares equal to the
    // canonical paths a server sends back.
    let to = match to.parent().and_then(|p| fs::canonicalize(p).ok()) {
        Some(parent) => parent.join(to.file_name().unwrap_or_default()),
        None => to,
    };
    if to.exists() {
        return Err(Error::Config(format!(
            "{} already exists; colab does not overwrite files",
            to.display()
        )));
    }
    let root = match spec.root {
        Some(root) => fs::canonicalize(spec.cwd.join(root)).map_err(|e| Error::io_at(root, e))?,
        None => project_root(&from).unwrap_or_else(|| spec.cwd.to_path_buf()),
    };
    let server = if spec.no_lsp {
        None
    } else {
        match spec.lsp {
            Some(command) => Some(ServerCommand::parse(command)?),
            None => ServerCommand::default_for(&from),
        }
    };
    let mut warnings = Vec::new();
    let (changes, label) = match &server {
        Some(server) => {
            info!("Moving via `{}` in {}", server.display(), root.display());
            let outcome = colab_lsp_client::will_rename(
                server,
                &MoveRequest {
                    root: root.clone(),
                    from: from.clone(),
                    to: to.clone(),
                    timeout: Duration::from_secs(spec.timeout),
                    settle: Duration::from_millis(500),
                },
            )?;
            if !outcome.references_updated {
                warnings.push(format!(
                    "`{}` does not update references when a file moves (no workspace.fileOperations.willRename); only the file was moved",
                    server.program
                ));
            }
            (outcome.changes, format!("move via {}", server.program))
        }
        None => {
            if !spec.no_lsp {
                warnings.push(format!(
                    "no default language server for {}; only the file was moved (pass `lsp` to update references)",
                    from.display()
                ));
            }
            let set = ChangeSet {
                ops: vec![FileOp::Move { from, to }],
                edits: Vec::new(),
            };
            (set, "move via git".to_string())
        }
    };
    Ok(MovePlan {
        changes,
        label,
        warnings,
        server,
        root,
    })
}
