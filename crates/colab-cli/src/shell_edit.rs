//! `colab hook pre-bash`: refuse a shell command that edits tracked files.
//!
//! An agent that rewrites a file with `sed -i`, `perl -pi`, a `>`
//! redirect, `tee`, or a Python / Node script that writes files skips the
//! post-edit hook and its harness's record of what changed, and never
//! reaches for colab. This module reads such a command the way a shell
//! would, closely enough to find what it writes ([`writes`]); the hook
//! then asks git which of those paths are tracked, and refuses the
//! command only when one is, naming each and what to use instead.
//!
//! It errs towards letting a command run: a write it cannot place (a path
//! in a variable, a script it cannot see) is allowed, except an in-place
//! `sed` / `perl` run through `xargs` or `find -exec` inside a work tree,
//! where the files are almost always git's own. A command carrying the
//! comment [`ALLOW`] always runs.
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The comment that lets a shell edit through.
pub const ALLOW: &str = "colab:allow-shell-edit";

/// A write the command would make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Write {
    /// How it writes, as the model should read it (`sed -i`).
    pub how: &'static str,
    pub target: Target,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A word of the command (or a script's string literal), resolved
    /// against the directory the command is in by then.
    Path { word: String, cwd: PathBuf },
    /// Files the command finds for itself (`xargs`, `find -exec`) under
    /// this directory.
    Found { cwd: PathBuf },
}

/// Why the command should not run, or `None` to let it.
pub fn verdict(command: &str, cwd: &Path) -> Option<String> {
    if command.contains(ALLOW) {
        return None;
    }
    let mut hits: Vec<String> = Vec::new();
    for w in writes(command, cwd) {
        let hit = match &w.target {
            Target::Path { word, cwd } => {
                tracked(word, cwd).then(|| format!("`{}` on {word}", w.how))
            }
            Target::Found { cwd } => in_work_tree(cwd)
                .then(|| format!("`{}` over files found in {}", w.how, cwd.display())),
        };
        if let Some(hit) = hit
            && !hits.contains(&hit)
        {
            hits.push(hit);
        }
    }
    if hits.is_empty() {
        return None;
    }
    let shown = hits.len().min(5);
    let mut list = hits[..shown].join(", ");
    if hits.len() > shown {
        let _ = write!(list, " and {} more", hits.len() - shown);
    }
    Some(format!(
        "colab: this command edits tracked files through the shell ({list}). \
         Shell edits skip the post-edit checks and Claude Code's record of what changed. \
         Use Edit or Write for a change in one place, and colab for the same change in several \
         (the colab MCP tools: colab_find, colab_preview, then colab_apply; or \
         `colab refactor -e \"match …\" --write <dir>`). If the shell really is the right tool \
         (a generated file, a binary), add the comment `# {ALLOW}` to the command and run it again."
    ))
}

/// Every write `command` makes, run from `cwd`, as far as reading it can
/// tell. Pure: nothing is run and no file is looked at.
pub fn writes(command: &str, cwd: &Path) -> Vec<Write> {
    let mut out = Vec::new();
    let mut cwd = cwd.to_path_buf();
    for pipeline in pipelines(&lex(command)) {
        let mut previous_body: Option<String> = None;
        let alone = pipeline.len() == 1;
        for simple in &pipeline {
            for (op, target) in &simple.redirects {
                if matches!(*op, ">" | ">>") {
                    out.push(path_write(
                        if *op == ">" { ">" } else { ">>" },
                        target,
                        &cwd,
                    ));
                }
            }
            let words = command_words(&simple.words);
            if let Some(name) = words.first().map(|w| basename(w)) {
                let args = &words[1..];
                match name {
                    "cd" | "pushd" if alone => {
                        let dir = args.first().map_or("~", String::as_str);
                        cwd = resolve(dir, &cwd);
                    }
                    "sed" | "gsed" if args.iter().any(|a| sed_in_place(a)) => {
                        out.extend(positional(args).map(|w| path_write("sed -i", w, &cwd)));
                    }
                    "perl" if args.iter().any(|a| perl_in_place(a)) => {
                        out.extend(positional(args).map(|w| path_write("perl -i", w, &cwd)));
                    }
                    "tee" => {
                        out.extend(positional(args).map(|w| path_write("tee", w, &cwd)));
                    }
                    "xargs" | "find" if runs_in_place(args) => out.push(Write {
                        how: if name == "xargs" {
                            "xargs sed -i"
                        } else {
                            "find -exec sed -i"
                        },
                        target: Target::Found { cwd: cwd.clone() },
                    }),
                    n if n.starts_with("python") || n == "node" => {
                        let flag = if n == "node" { "-e" } else { "-c" };
                        let script = args
                            .iter()
                            .position(|a| a == flag)
                            .and_then(|i| args.get(i + 1).cloned())
                            .or_else(|| simple.body.clone())
                            .or_else(|| previous_body.clone());
                        let how = if n == "node" {
                            "a Node script"
                        } else {
                            "a Python script"
                        };
                        if let Some(script) = script {
                            out.extend(
                                script_targets(&script)
                                    .into_iter()
                                    .map(|w| path_write(how, &w, &cwd)),
                            );
                        }
                    }
                    _ => {}
                }
            }
            previous_body.clone_from(&simple.body);
        }
    }
    out
}

fn path_write(how: &'static str, word: &str, cwd: &Path) -> Write {
    Write {
        how,
        target: Target::Path {
            word: word.to_string(),
            cwd: cwd.to_path_buf(),
        },
    }
}

/// The words after leading assignments and wrappers (`env`, `sudo`, …).
fn command_words(words: &[String]) -> &[String] {
    let mut i = 0;
    while let Some(w) = words.get(i) {
        let assignment = w.split_once('=').is_some_and(|(k, _)| {
            !k.is_empty() && k.chars().all(|c| c.is_alphanumeric() || c == '_')
        });
        if assignment
            || matches!(
                w.as_str(),
                "env" | "sudo" | "command" | "exec" | "nice" | "time"
            )
        {
            i += 1;
        } else {
            break;
        }
    }
    &words[i..]
}

fn basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// The words that are not options: the files, and the script, which git
/// will not find tracked.
fn positional(args: &[String]) -> impl Iterator<Item = &str> {
    args.iter()
        .map(String::as_str)
        .filter(|a| !a.is_empty() && !a.starts_with('-'))
}

fn sed_in_place(arg: &str) -> bool {
    if arg == "--in-place" || arg.starts_with("--in-place=") {
        return true;
    }
    let Some(flags) = arg.strip_prefix('-') else {
        return false;
    };
    if flags.starts_with('-') {
        return false;
    }
    // `-i`, `-i.bak`, and `-i` after flags that take no value (`-Ei`).
    flags
        .chars()
        .take_while(|c| *c == 'i' || "nrEsuzl".contains(*c))
        .any(|c| c == 'i')
}

fn perl_in_place(arg: &str) -> bool {
    let Some(flags) = arg.strip_prefix('-') else {
        return false;
    };
    // `-pi`, `-i.bak`, `-0777 -pi`, `-pie`: an `i` before any `e`.
    !flags.starts_with('-') && flags.split('e').next().is_some_and(|f| f.contains('i'))
}

/// `xargs [opts] sed -i …` or `find … -exec sed -i …`.
fn runs_in_place(args: &[String]) -> bool {
    args.iter().enumerate().any(|(i, a)| {
        let rest = &args[i + 1..];
        match basename(a) {
            "sed" | "gsed" => rest.iter().any(|r| sed_in_place(r)),
            "perl" => rest.iter().any(|r| perl_in_place(r)),
            _ => false,
        }
    })
}

/// The paths a script that writes files names: the string literals on its
/// writing lines, else (it writes through a variable) every literal.
/// Nothing when it does not write.
fn script_targets(script: &str) -> Vec<String> {
    let writing: Vec<&str> = script.lines().filter(|l| writes_a_file(l)).collect();
    if writing.is_empty() {
        return Vec::new();
    }
    let on_lines: Vec<String> = writing
        .iter()
        .flat_map(|l| literals(l))
        .filter(|s| path_like(s))
        .collect();
    if !on_lines.is_empty() {
        return on_lines;
    }
    let mut all: Vec<String> = script
        .lines()
        .flat_map(literals)
        .filter(|s| path_like(s))
        .collect();
    all.dedup();
    all.truncate(50);
    all
}

fn writes_a_file(line: &str) -> bool {
    const CALLS: &[&str] = &[
        ".write_text(",
        ".write_bytes(",
        "inplace=True",
        "os.replace(",
        "shutil.move(",
        "shutil.copy",
        "writeFileSync(",
        "writeFile(",
        "appendFileSync(",
    ];
    CALLS.iter().any(|c| line.contains(c))
        || line
            .match_indices("open(")
            .any(|(i, _)| has_write_mode(&line[i..]))
}

/// A mode argument that writes (`, 'w'`, `mode="a+"`) in an `open(` call.
fn has_write_mode(call: &str) -> bool {
    let chars: Vec<char> = call.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == ')' {
            return false;
        }
        if chars[i] == ',' || chars[i] == '=' {
            let mut j = i + 1;
            while chars.get(j) == Some(&' ') {
                j += 1;
            }
            if let Some(&q) = chars.get(j).filter(|c| **c == '\'' || **c == '"') {
                let mode: String = chars[j + 1..].iter().take_while(|c| **c != q).collect();
                if !mode.is_empty()
                    && mode.len() <= 4
                    && mode.chars().all(|c| "rwabxt+".contains(c))
                    && mode.chars().any(|c| "wax+".contains(c))
                {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

/// The single- and double-quoted strings on one line.
fn literals(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\'' && c != '"' {
            continue;
        }
        let mut s = String::new();
        let mut closed = false;
        while let Some(d) = chars.next() {
            if d == '\\' {
                if let Some(e) = chars.next() {
                    s.push(e);
                }
            } else if d == c {
                closed = true;
                break;
            } else {
                s.push(d);
            }
        }
        if closed {
            out.push(s);
        }
    }
    out
}

fn path_like(s: &str) -> bool {
    !s.is_empty()
        && s.len() < 256
        && s != "."
        && s != ".."
        && (s.contains('/') || s.contains('.'))
        && !s.contains(char::is_whitespace)
        && !s.contains("://")
        && !s.contains(['$', '{', '}', '%', '\\'])
}

fn resolve(word: &str, cwd: &Path) -> PathBuf {
    let home = || std::env::var_os("HOME").map_or_else(PathBuf::new, PathBuf::from);
    if word == "~" {
        home()
    } else if let Some(rest) = word.strip_prefix("~/") {
        home().join(rest)
    } else {
        cwd.join(word)
    }
}

/// Whether git tracks the file (or, for a directory or glob, anything in
/// it) that `word` names from `cwd`.
fn tracked(word: &str, cwd: &Path) -> bool {
    if word.contains('$') || word.starts_with("/dev/") {
        return false;
    }
    let path = resolve(word, cwd);
    let mut base = path.parent().map_or_else(PathBuf::new, Path::to_path_buf);
    while !base.is_dir() {
        match base.parent() {
            Some(parent) => base = parent.to_path_buf(),
            None => return false,
        }
    }
    Command::new("git")
        .arg("-C")
        .arg(&base)
        .args(["ls-files", "--error-unmatch", "--"])
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn in_work_tree(cwd: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--is-inside-work-tree"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success() && o.stdout.starts_with(b"true"))
}

// --- reading the command ---------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Word(String),
    Op(&'static str),
    /// A here-document's text, where its `<<` was.
    Body(String),
}

#[derive(Debug, Default)]
struct Simple {
    words: Vec<String>,
    redirects: Vec<(&'static str, String)>,
    /// What it reads on stdin, from a here-document or here-string.
    body: Option<String>,
}

/// The command's pipelines, each a list of simple commands.
fn pipelines(toks: &[Tok]) -> Vec<Vec<Simple>> {
    let mut out = Vec::new();
    let mut pipeline = Vec::new();
    let mut simple = Simple::default();
    let mut it = toks.iter().peekable();
    while let Some(tok) = it.next() {
        match tok {
            Tok::Word(w) => simple.words.push(w.clone()),
            Tok::Body(b) => simple.body = Some(b.clone()),
            Tok::Op(op @ (">" | ">>" | ">&" | "<" | "<<<")) => {
                if let Some(Tok::Word(target)) = it.peek() {
                    let target = target.clone();
                    it.next();
                    match *op {
                        ">" | ">>" => simple.redirects.push((op, target)),
                        "<<<" => simple.body = Some(target),
                        _ => {}
                    }
                }
            }
            Tok::Op("|") => pipeline.push(std::mem::take(&mut simple)),
            Tok::Op(_) => {
                pipeline.push(std::mem::take(&mut simple));
                out.push(std::mem::take(&mut pipeline));
            }
        }
    }
    pipeline.push(simple);
    out.push(pipeline);
    out.into_iter()
        .map(|p| {
            p.into_iter()
                .filter(|s| !s.words.is_empty() || !s.redirects.is_empty())
                .collect()
        })
        .filter(|p: &Vec<Simple>| !p.is_empty())
        .collect()
}

/// Words and operators, quotes removed, here-documents' text in place.
fn lex(src: &str) -> Vec<Tok> {
    let chars: Vec<char> = src.chars().collect();
    let mut toks = Vec::new();
    // Here-documents opened on the current line: (token index, delimiter, `<<-`).
    let mut pending: Vec<(usize, String, bool)> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '\n' => {
                toks.push(Tok::Op("\n"));
                i += 1;
                for (idx, delim, strip) in pending.drain(..) {
                    let mut body = String::new();
                    while i < chars.len() {
                        let end = chars[i..]
                            .iter()
                            .position(|&c| c == '\n')
                            .map_or(chars.len(), |p| i + p);
                        let line: String = chars[i..end].iter().collect();
                        i = (end + 1).min(chars.len());
                        let bare = if strip {
                            line.trim_start_matches('\t')
                        } else {
                            &line
                        };
                        if bare == delim {
                            break;
                        }
                        body.push_str(&line);
                        body.push('\n');
                    }
                    toks[idx] = Tok::Body(body);
                }
            }
            ' ' | '\t' | '\r' => i += 1,
            '\\' if next == Some('\n') => i += 2,
            '#' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            ';' | '(' | ')' => {
                toks.push(Tok::Op(";"));
                i += 1;
            }
            '&' if next == Some('&') => {
                toks.push(Tok::Op("&&"));
                i += 2;
            }
            '&' if next == Some('>') => {
                // `&>file`: stdout and stderr to the file.
                toks.push(Tok::Op(">"));
                i += 2;
                if chars.get(i) == Some(&'>') {
                    i += 1;
                }
            }
            '&' => {
                toks.push(Tok::Op(";"));
                i += 1;
            }
            '|' if next == Some('|') => {
                toks.push(Tok::Op("||"));
                i += 2;
            }
            '|' => {
                toks.push(Tok::Op("|"));
                i += 1;
                if chars.get(i) == Some(&'&') {
                    i += 1;
                }
            }
            '>' => {
                i += 1;
                let mut op = ">";
                if chars.get(i) == Some(&'>') {
                    op = ">>";
                    i += 1;
                }
                if chars.get(i) == Some(&'&') {
                    op = ">&";
                    i += 1;
                } else if chars.get(i) == Some(&'|') {
                    i += 1;
                }
                toks.push(Tok::Op(op));
            }
            '<' if next == Some('<') && chars.get(i + 2) == Some(&'<') => {
                toks.push(Tok::Op("<<<"));
                i += 3;
            }
            '<' if next == Some('<') => {
                i += 2;
                let strip = chars.get(i) == Some(&'-');
                if strip {
                    i += 1;
                }
                while matches!(chars.get(i), Some(' ' | '\t')) {
                    i += 1;
                }
                let (delim, end) = word(&chars, i);
                i = end;
                pending.push((toks.len(), delim, strip));
                toks.push(Tok::Body(String::new()));
            }
            '<' => {
                toks.push(Tok::Op("<"));
                i += 1;
                if chars.get(i) == Some(&'(') {
                    // `<(cmd)`: read what is inside as a command of its own.
                    toks.push(Tok::Op(";"));
                    i += 1;
                }
            }
            _ => {
                let (w, end) = word(&chars, i);
                i = end;
                toks.push(Tok::Word(w));
            }
        }
    }
    toks
}

/// One word from `start`, quotes removed: its text and where it ends.
fn word(chars: &[char], start: usize) -> (String, usize) {
    let mut s = String::new();
    let mut i = start;
    while let Some(&c) = chars.get(i) {
        match c {
            ' ' | '\t' | '\r' | '\n' | ';' | '&' | '|' | '<' | '>' | '(' | ')' => break,
            '\'' => {
                i += 1;
                while let Some(&d) = chars.get(i) {
                    i += 1;
                    if d == '\'' {
                        break;
                    }
                    s.push(d);
                }
            }
            '"' => {
                i += 1;
                while let Some(&d) = chars.get(i) {
                    i += 1;
                    match d {
                        '"' => break,
                        '\\' if matches!(chars.get(i), Some('"' | '\\' | '$' | '`')) => {
                            s.push(chars[i]);
                            i += 1;
                        }
                        _ => s.push(d),
                    }
                }
            }
            '\\' => {
                if let Some(&d) = chars.get(i + 1)
                    && d != '\n'
                {
                    s.push(d);
                }
                i += 2;
            }
            '$' if chars.get(i + 1) == Some(&'(') => {
                // `$(…)` stays in the word, as written.
                let mut depth = 0;
                while let Some(&d) = chars.get(i) {
                    s.push(d);
                    i += 1;
                    match d {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
            '`' => {
                s.push(c);
                i += 1;
                while let Some(&d) = chars.get(i) {
                    s.push(d);
                    i += 1;
                    if d == '`' {
                        break;
                    }
                }
            }
            _ => {
                s.push(c);
                i += 1;
            }
        }
    }
    (s, i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(command: &str) -> Vec<(&'static str, String)> {
        writes(command, Path::new("/repo"))
            .into_iter()
            .map(|w| match w.target {
                Target::Path { word, cwd } => (w.how, cwd.join(word).display().to_string()),
                Target::Found { cwd } => (w.how, format!("{}/…", cwd.display())),
            })
            .collect()
    }

    fn pair(how: &'static str, path: &str) -> (&'static str, String) {
        (how, path.to_string())
    }

    #[test]
    fn in_place_sed_and_perl_name_their_files() {
        assert_eq!(
            found("sed -i '' -e 's/a/b/' src/a.rs src/b.rs"),
            [
                pair("sed -i", "/repo/s/a/b/"),
                pair("sed -i", "/repo/src/a.rs"),
                pair("sed -i", "/repo/src/b.rs")
            ]
        );
        assert_eq!(
            found("sed -Ei.bak 's/x/y/' f.go"),
            [pair("sed -i", "/repo/s/x/y/"), pair("sed -i", "/repo/f.go")]
        );
        assert_eq!(
            found("perl -0777 -pi -e 's/x/y/g' lib.rs").last(),
            Some(&pair("perl -i", "/repo/lib.rs"))
        );
        assert!(
            found("sed -n '1,20p' src/a.rs").is_empty(),
            "reading is fine"
        );
        assert!(found("perl -ne 'print' a.rs").is_empty());
    }

    #[test]
    fn redirects_and_tee_write_their_target() {
        assert_eq!(
            found("cat > docs/a.md <<'EOF'\n# A\nEOF\necho done"),
            [pair(">", "/repo/docs/a.md")]
        );
        assert_eq!(
            found("echo x >> .gitignore 2>&1"),
            [pair(">>", "/repo/.gitignore")]
        );
        assert_eq!(
            found("printf x | tee -a notes.txt"),
            [pair("tee", "/repo/notes.txt")]
        );
        assert!(found("cargo test 2>&1 | tail -5").is_empty());
        assert!(found("ls >/dev/null").iter().all(|(_, p)| p == "/dev/null"));
    }

    #[test]
    fn cd_moves_the_rest_of_the_command() {
        assert_eq!(
            found("cd crates/x && sed -i 's/a/b/' lib.rs; cd /tmp && echo > out"),
            [
                pair("sed -i", "/repo/crates/x/s/a/b/"),
                pair("sed -i", "/repo/crates/x/lib.rs"),
                pair(">", "/tmp/out")
            ]
        );
    }

    #[test]
    fn a_script_that_writes_names_its_paths() {
        let heredoc = "python3 - <<'PY'\nimport pathlib\np = pathlib.Path('crates/a.rs')\ns = p.read_text().replace('foo.bar', 'baz')\np.write_text(s)\nPY";
        assert_eq!(
            found(heredoc),
            [
                pair("a Python script", "/repo/crates/a.rs"),
                pair("a Python script", "/repo/foo.bar")
            ]
        );
        let inline = r#"python3 -c "open('src/m.py', 'w').write('x')""#;
        assert_eq!(found(inline), [pair("a Python script", "/repo/src/m.py")]);
        let piped = "cat <<EOF | python3\nopen('a.txt','a').write('x')\nEOF";
        assert_eq!(found(piped), [pair("a Python script", "/repo/a.txt")]);
        assert!(
            found("python3 -c \"print(open('Cargo.toml').read())\"").is_empty(),
            "a script that only reads"
        );
        assert_eq!(
            found("node -e \"fs.writeFileSync('pkg/x.json', s)\""),
            [pair("a Node script", "/repo/pkg/x.json")]
        );
    }

    #[test]
    fn xargs_and_find_edit_what_they_find() {
        assert_eq!(
            found("git grep -l foo | xargs sed -i 's/foo/bar/g'"),
            [pair("xargs sed -i", "/repo/…")]
        );
        assert_eq!(
            found("find . -name '*.rs' -exec perl -pi -e 's/a/b/' {} +"),
            [pair("find -exec sed -i", "/repo/…")]
        );
        assert!(found("git grep -l foo | xargs wc -l").is_empty());
    }

    #[test]
    fn the_verdict_needs_a_tracked_file_and_respects_the_comment() {
        let repo = std::env::temp_dir().join(format!("colab-shell-edit-{}", std::process::id()));
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&repo)
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        };
        git(&["init", "-q"]);
        std::fs::write(repo.join("tracked.rs"), "fn a() {}\n").unwrap();
        std::fs::write(repo.join("scratch.txt"), "x\n").unwrap();
        git(&["add", "tracked.rs"]);

        let reason = verdict("sed -i 's/a/b/' tracked.rs scratch.txt", &repo).unwrap();
        assert!(reason.contains("`sed -i` on tracked.rs"), "{reason}");
        assert!(!reason.contains("scratch.txt"), "{reason}");
        assert!(reason.contains(ALLOW));
        assert!(
            verdict("sed -i 's/a/b/' scratch.txt", &repo).is_none(),
            "untracked"
        );
        assert!(verdict("echo x > /tmp/elsewhere.txt", &repo).is_none());
        assert!(verdict(&format!("sed -i 's/a/b/' tracked.rs # {ALLOW}"), &repo).is_none());
        assert!(verdict("git grep -l a | xargs sed -i 's/a/b/'", &repo).is_some());
        assert!(
            verdict(
                "xargs sed -i 's/a/b/'",
                &std::env::temp_dir().join("no-such-repo")
            )
            .is_none()
        );
        std::fs::remove_dir_all(&repo).ok();
    }
}
