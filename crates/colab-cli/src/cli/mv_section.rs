//! `colab mv-section`: move a Markdown section to another file, repointing
//! the links to it.

use super::*;

/// Whether any Markdown file under `root` other than `skip` contains
/// `needle` (hidden directories and `target/` skipped), so a rule that
/// would match nothing is not generated.
pub(super) fn markdown_mentions(root: &Path, needle: &str, skip: &[&Path]) -> bool {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "target" {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .is_some_and(|e| e == "md" || e == "markdown")
                && !skip
                    .iter()
                    .any(|s| fs::canonicalize(&path).ok().as_deref() == Some(*s))
                && fs::read_to_string(&path).is_ok_and(|text| text.contains(needle))
            {
                return true;
            }
        }
    }
    false
}

/// `path` relative to `base` (a directory), with `..` as needed.
pub(super) fn relative_path(base: &Path, path: &Path) -> String {
    let base: Vec<_> = base.components().collect();
    let path_parts: Vec<_> = path.components().collect();
    let common = base
        .iter()
        .zip(&path_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let mut out: Vec<String> = vec!["..".into(); base.len() - common];
    out.extend(
        path_parts[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    out.join("/")
}

/// `colab mv-section`: a section moved between files, as a generated
/// script — an insert in the target, a delete in the source, and link
/// rewrites — run like `refactor`, so conflicts and the link check apply.
pub(super) fn run_mv_section(args: MvSectionArgs) -> Result<u8> {
    use colab_lang_markdown::moving::{Placement, plan};
    change_working_dir(&args.change_dir)?;
    let cwd = fs::canonicalize(".").map_err(io_to_error)?;
    let canonical = |p: &Path| fs::canonicalize(p).map_err(|e| Error::io_at(p, e));
    let (source_path, target_path) = (canonical(&args.file)?, canonical(&args.to)?);
    if source_path == target_path {
        return Err(Error::Config(
            "the section is already in that file; to move it within a file, use `markdown::section` rules".into(),
        ));
    }
    let rel = |p: &Path| {
        p.strip_prefix(&cwd)
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .map_err(|_| Error::Config(format!("{} is outside the working directory", p.display())))
    };
    let (source_rel, target_rel) = (rel(&source_path)?, rel(&target_path)?);
    let source = fs::read_to_string(&source_path).map_err(|e| Error::io_at(&source_path, e))?;
    let target = fs::read_to_string(&target_path).map_err(|e| Error::io_at(&target_path, e))?;
    let placement = match (&args.under, &args.before) {
        (Some(parent), _) => Placement::Under(parent),
        (None, Some(sibling)) => Placement::Before(sibling),
        (None, None) => Placement::End,
    };
    let mut moved = plan(&source, &args.section, &target, placement).map_err(Error::Config)?;

    let quote = |text: &str| -> Result<String> {
        if text.contains("\"##") {
            return Err(Error::Config(
                "the section holds `\"##`, which the generated script cannot quote".into(),
            ));
        }
        Ok(format!("r##\"{text}\"##"))
    };
    let dir = |p: &Path| p.parent().unwrap_or(Path::new("")).to_path_buf();
    let to_target = relative_path(&dir(&source_path), &target_path);
    let to_source = relative_path(&dir(&target_path), &source_path);
    // The section's own links start from the target now: an anchor to a
    // section left behind gains the path back to the source, a relative
    // file link is recomputed from the target's directory, and an anchor
    // inside the moved section stays as it is.
    let same_dir = dir(&source_path) == dir(&target_path);
    moved.body = colab_lang_markdown::moving::rebase_links(&moved.body, |url| {
        if url.contains("://") || url.starts_with('/') || url.starts_with("mailto:") {
            return None;
        }
        if let Some(anchor) = url.strip_prefix('#') {
            return (!moved.own_slugs.iter().any(|s| s == anchor))
                .then(|| format!("{to_source}{url}"));
        }
        if same_dir || url.is_empty() {
            return None;
        }
        let (path, fragment) = match url.split_once('#') {
            Some((p, f)) => (p, format!("#{f}")),
            None => (url, String::new()),
        };
        let resolved = fs::canonicalize(dir(&source_path).join(path)).ok()?;
        Some(format!(
            "{}{fragment}",
            relative_path(&dir(&target_path), &resolved)
        ))
    });
    let mut script = format!(
        "// colab mv-section: `{}` from {source_rel} to {target_rel}.\nrefactor \"mv-section\" {{\n",
        args.section
    );
    let _ = writeln!(
        script,
        "    match markdown::section {} in \"{target_rel}\" {{ insert {} }}",
        quote(&moved.insert_address)?,
        quote(&moved.body)?
    );
    let _ = writeln!(
        script,
        "    match markdown::section {} in \"{source_rel}\" {{ delete }}",
        quote(&args.section)?
    );
    // Links to the section from either file now cross the move; links
    // from elsewhere are caught by the link check.
    let old = format!("#{}", moved.old_slug);
    if source.contains(&format!("]({old}")) {
        let _ = writeln!(
            script,
            "    match markdown::link \"{old}\" in \"{source_rel}\" {{ replace \"{to_target}#{}\" }}",
            moved.new_slug
        );
    }
    let from_target = format!("{to_source}{old}");
    if target.contains(&format!("]({from_target}")) {
        let _ = writeln!(
            script,
            "    match markdown::link \"{from_target}\" in \"{target_rel}\" {{ replace \"#{}\" }}",
            moved.new_slug
        );
    }
    // Links from other files: when the two files share a directory, the
    // file name is all that changes, wherever the link is from. Otherwise
    // the path differs per linking file, and the link check reports them.
    if same_dir {
        let (Some(from), Some(to)) = (source_path.file_name(), target_path.file_name()) else {
            return Err(Error::Config("a file without a name".into()));
        };
        let needle = format!("{}{old}", from.to_string_lossy());
        if markdown_mentions(&cwd, &needle, &[&source_path, &target_path]) {
            let _ = writeln!(
                script,
                "    match markdown::link \"{}{old}\" {{ replace \"{}#{}\" }}",
                from.to_string_lossy(),
                to.to_string_lossy(),
                moved.new_slug
            );
        }
    }
    script.push_str("}\n");
    if args.show_script {
        print!("{script}");
        return Ok(0);
    }

    let format = clap::ValueEnum::to_possible_value(&args.format)
        .map_or_else(|| "human".into(), |v| v.get_name().to_string());
    let mut argv: Vec<String> = vec![
        "refactor".into(),
        "-e".into(),
        String::new(),
        "--format".into(),
        format,
    ];
    if args.check {
        argv.push("--check".into());
    } else if args.write {
        argv.push("--write".into());
    } else if args.dry_run {
        argv.push("--dry-run".into());
    }
    if args.allow_conflicts {
        argv.push("--allow-conflicts".into());
    }
    // The whole tree, so the link check sees every Markdown file.
    argv.extend(["--".into(), ".".into()]);
    let refactor = RefactorArgs::try_parse_from(&argv).map_err(|e| Error::Config(e.to_string()))?;
    let backends = default_backends();
    let refactoring = codemod::compile_at_source(&script, &cwd.join("<mv-section>"), &backends)?;
    run_compiled(&refactoring, &refactor)
}
