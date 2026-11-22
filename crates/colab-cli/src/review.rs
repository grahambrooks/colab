//! `colab review`: a large run, sliced for review.
//!
//! A run that touches thousands of files is one unreviewable diff. Recorded
//! with `--format ndjson --detail diff`, its log holds one event per changed
//! file (the rules that changed it, and the diff) and a summary naming each
//! rule. `colab review <log>` groups the files by rule: how many each
//! changed, the lines they add and remove, the most-edited files, and how
//! edits spread across files, so a reviewer can sample. `--rule N` pages
//! through that rule's diffs. Nothing is interactive, so an agent reads it
//! the same way a person does.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use colab_core::{Error, Result};
use serde_json::{Value, json};

#[derive(Parser, Debug)]
pub struct ReviewArgs {
    /// The run's log: the output of `colab refactor` (or `colab fix`) with
    /// `--format ndjson --detail diff`. `-` reads it from stdin.
    log: PathBuf,

    /// Page through the diffs of this rule, numbered as in the overview
    /// (from 1).
    #[arg(long, value_name = "N")]
    rule: Option<usize>,

    /// The page of `--rule`'s diffs to show (from 1).
    #[arg(long, value_name = "P", default_value_t = 1, requires = "rule")]
    page: usize,

    /// Diffs per page.
    #[arg(long = "per-page", value_name = "K", default_value_t = 10)]
    per_page: usize,

    /// Most-edited files listed per rule.
    #[arg(long, value_name = "N", default_value_t = 5)]
    top: usize,

    /// `human`: an overview, then the page. `json`: one document.
    #[arg(long, value_enum, default_value_t = ReviewFormat::Human)]
    format: ReviewFormat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum ReviewFormat {
    Human,
    Json,
}

/// One changed file, as the log records it.
#[derive(Debug, Clone)]
struct File {
    path: String,
    rules: Vec<usize>,
    diff: Option<String>,
    added: usize,
    removed: usize,
    /// Lines each rule added and removed here, when the run attributed
    /// them (`rule_lines`); `None` when it did not.
    per_rule: Option<BTreeMap<usize, (usize, usize)>>,
}

impl File {
    /// What `rule` added and removed here: its own lines when the log
    /// attributes them, else the whole file's.
    fn lines_of(&self, rule: usize) -> (usize, usize) {
        match &self.per_rule {
            Some(per) => per.get(&rule).copied().unwrap_or_default(),
            None => (self.added, self.removed),
        }
    }

    /// Lines `rule` changed here, added and removed together.
    fn size(&self, rule: usize) -> usize {
        let (a, r) = self.lines_of(rule);
        a + r
    }

    /// `true` when other rules changed this file too and the log cannot
    /// say whose lines are whose.
    fn shared(&self) -> bool {
        self.rules.len() > 1 && self.per_rule.is_none()
    }
}

/// The parsed log.
#[derive(Debug, Default)]
struct Log {
    files: Vec<File>,
    /// Rule labels by index, from the summary event.
    rules: BTreeMap<usize, String>,
    /// Whether any file event carried a diff.
    diffs: bool,
}

/// Lines a unified diff adds and removes (headers not counted).
fn count(diff: &str) -> (usize, usize) {
    let mut added = 0;
    let mut removed = 0;
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    (added, removed)
}

fn parse(text: &str) -> Result<Log> {
    let mut log = Log::default();
    let mut summary = false;
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let event: Value = serde_json::from_str(line).map_err(|e| {
            Error::Config(format!(
                "line {}: not JSON ({e}); colab review reads the output of `--format ndjson --detail diff`",
                n + 1
            ))
        })?;
        match event["type"].as_str() {
            Some("file") => {
                let diff = event["diff"].as_str().map(str::to_string);
                let (added, removed) = diff.as_deref().map_or_default(count);
                log.diffs |= diff.is_some();
                log.files.push(File {
                    path: event["path"].as_str().unwrap_or("").to_string(),
                    rules: event["rules"].as_array().map_or_default(|a| {
                        a.iter()
                            .filter_map(serde_json::Value::as_u64)
                            .filter_map(|v| usize::try_from(v).ok())
                            .collect()
                    }),
                    diff,
                    added,
                    removed,
                    per_rule: event["rule_lines"].as_array().map(|lines| {
                        lines
                            .iter()
                            .filter_map(|l| {
                                let count = |key: &str| usize::try_from(l[key].as_u64()?).ok();
                                let rule = count("rule")?;
                                let added = count("added")?;
                                let removed = count("removed")?;
                                Some((rule, (added, removed)))
                            })
                            .collect()
                    }),
                });
            }
            Some("summary") => {
                summary = true;
                for rule in event["rules"].as_array().into_iter().flatten() {
                    let i = rule["i"].as_u64().and_then(|i| usize::try_from(i).ok());
                    if let (Some(i), Some(label)) = (i, rule["rule"].as_str()) {
                        log.rules.insert(i, label.to_string());
                    }
                }
            }
            // File operations and anything newer: not part of the review.
            _ => {}
        }
    }
    if !summary {
        return Err(Error::Config(
            "the log has no summary event; record the whole run with `--format ndjson --detail diff`"
                .into(),
        ));
    }
    Ok(log)
}

/// Histogram buckets, by lines changed in a file.
const BUCKETS: &[(&str, usize, usize)] = &[
    ("1-2", 1, 2),
    ("3-5", 3, 5),
    ("6-20", 6, 20),
    ("21-100", 21, 100),
    ("101+", 101, usize::MAX),
];

fn histogram(files: &[&File], rule: usize) -> Vec<(&'static str, usize)> {
    BUCKETS
        .iter()
        .map(|(name, lo, hi)| {
            (
                *name,
                files
                    .iter()
                    .filter(|f| (*lo..=*hi).contains(&f.size(rule)))
                    .count(),
            )
        })
        .filter(|(_, n)| *n > 0)
        .collect()
}

/// The review as one JSON document; the human form is printed from it.
fn review(log: &Log, args: &ReviewArgs) -> Result<Value> {
    let mut rules = Vec::new();
    for (&i, label) in &log.rules {
        let files: Vec<&File> = log.files.iter().filter(|f| f.rules.contains(&i)).collect();
        let mut top = files.clone();
        top.sort_by(|a, b| b.size(i).cmp(&a.size(i)).then_with(|| a.path.cmp(&b.path)));
        let mut rule = json!({
            "i": i,
            "rule": label,
            "files": files.len(),
        });
        // A file's diff is the whole run's change to it, so in a file that
        // other rules changed too the lines are not this rule's alone.
        let shared = files.iter().filter(|f| f.shared()).count();
        if shared > 0 {
            rule["shared"] = json!(shared);
        }
        if log.diffs {
            rule["added"] = json!(files.iter().map(|f| f.lines_of(i).0).sum::<usize>());
            rule["removed"] = json!(files.iter().map(|f| f.lines_of(i).1).sum::<usize>());
            rule["top"] = json!(
                top.iter()
                    .take(args.top)
                    .map(|f| {
                        let (added, removed) = f.lines_of(i);
                        json!({"path": f.path, "added": added, "removed": removed})
                    })
                    .collect::<Vec<_>>()
            );
            rule["histogram"] = Value::Object(
                histogram(&files, i)
                    .into_iter()
                    .map(|(k, n)| (k.to_string(), json!(n)))
                    .collect(),
            );
        }
        rules.push(rule);
    }
    let mut doc = json!({
        "files": log.files.len(),
        "rules": rules,
    });
    if log.diffs {
        doc["added"] = json!(log.files.iter().map(|f| f.added).sum::<usize>());
        doc["removed"] = json!(log.files.iter().map(|f| f.removed).sum::<usize>());
    } else {
        doc["warnings"] = json!([
            "the log has no diffs, so there are no line counts or pages: record it with --format ndjson --detail diff"
        ]);
    }
    if let Some(n) = args.rule {
        let i = n
            .checked_sub(1)
            .filter(|i| log.rules.contains_key(i))
            .ok_or_else(|| {
                Error::Config(format!(
                    "--rule {n}: the log has rules 1 to {}",
                    log.rules.len()
                ))
            })?;
        let files: Vec<&File> = log.files.iter().filter(|f| f.rules.contains(&i)).collect();
        let per = args.per_page.max(1);
        let pages = files.len().div_ceil(per).max(1);
        if args.page == 0 || args.page > pages {
            return Err(Error::Config(format!(
                "--page {}: rule {n} has {pages} page(s) of {per}",
                args.page
            )));
        }
        let shown: Vec<Value> = files
            .iter()
            .skip((args.page - 1) * per)
            .take(per)
            .map(|f| {
                let mut v = json!({"path": f.path, "added": f.added, "removed": f.removed});
                if let Some(d) = &f.diff {
                    v["diff"] = json!(d);
                }
                v
            })
            .collect();
        doc["page"] = json!({
            "rule": i,
            "page": args.page,
            "pages": pages,
            "files": shown,
        });
    }
    Ok(doc)
}

fn human(doc: &Value, log_name: &str, out: &mut impl Write) -> io::Result<()> {
    let lines = |v: &Value| -> String {
        match (v["added"].as_u64(), v["removed"].as_u64()) {
            (Some(a), Some(r)) => format!(", +{a} -{r} lines"),
            _ => String::new(),
        }
    };
    let rules = doc["rules"].as_array().cloned().unwrap_or_default();
    writeln!(
        out,
        "{} file(s) changed by {} rule(s){}",
        doc["files"],
        rules.len(),
        lines(doc)
    )?;
    for rule in &rules {
        let n = rule["i"].as_u64().unwrap_or(0) + 1;
        let files = rule["files"].as_u64().unwrap_or(0);
        let label = rule["rule"].as_str().unwrap_or("");
        if files == 0 {
            writeln!(out, "rule {n}: 0 file(s) — {label} (matched nothing)")?;
            continue;
        }
        writeln!(out, "rule {n}: {files} file(s){} — {label}", lines(rule))?;
        if let Some(shared) = rule["shared"].as_u64() {
            writeln!(
                out,
                "  {shared} of them also changed by other rules, and the log does not split their lines, so they count for each"
            )?;
        }
        if let Some(top) = rule["top"].as_array().filter(|t| !t.is_empty()) {
            let shown: Vec<String> = top
                .iter()
                .map(|f| {
                    format!(
                        "{} (+{} -{})",
                        f["path"].as_str().unwrap_or(""),
                        f["added"],
                        f["removed"]
                    )
                })
                .collect();
            writeln!(out, "  most edited: {}", shown.join(", "))?;
        }
        if let Some(hist) = rule["histogram"].as_object().filter(|h| !h.is_empty()) {
            let shown: Vec<String> = BUCKETS
                .iter()
                .filter_map(|(name, _, _)| hist.get(*name).map(|n| format!("{name}: {n}")))
                .collect();
            writeln!(out, "  lines changed per file: {}", shown.join(" · "))?;
        }
    }
    for warning in doc["warnings"].as_array().into_iter().flatten() {
        writeln!(out, "warning: {}", warning.as_str().unwrap_or(""))?;
    }
    match doc.get("page") {
        Some(page) => {
            let n = page["rule"].as_u64().unwrap_or(0) + 1;
            let (p, pages) = (
                page["page"].as_u64().unwrap_or(1),
                page["pages"].as_u64().unwrap_or(1),
            );
            writeln!(out, "\nrule {n}, page {p} of {pages}:")?;
            for file in page["files"].as_array().into_iter().flatten() {
                match file["diff"].as_str() {
                    Some(diff) => write!(out, "{diff}")?,
                    None => writeln!(out, "{}", file["path"].as_str().unwrap_or(""))?,
                }
            }
            if p < pages {
                writeln!(
                    out,
                    "next: colab review {log_name} --rule {n} --page {}",
                    p + 1
                )?;
            }
        }
        None => {
            if let Some(first) = rules.iter().find(|r| r["files"].as_u64().unwrap_or(0) > 0) {
                let n = first["i"].as_u64().unwrap_or(0) + 1;
                writeln!(
                    out,
                    "next: colab review {log_name} --rule {n}   # page through its diffs"
                )?;
            }
        }
    }
    Ok(())
}

pub fn run(args: ReviewArgs) -> Result<u8> {
    let text = if args.log.as_os_str() == "-" {
        let mut text = String::new();
        io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| Error::io_at(&args.log, e))?;
        text
    } else {
        fs::read_to_string(&args.log).map_err(|e| Error::io_at(&args.log, e))?
    };
    let log = parse(&text)?;
    let doc = review(&log, &args)?;
    let mut out = io::stdout().lock();
    let written = match args.format {
        ReviewFormat::Json => writeln!(out, "{doc}"),
        ReviewFormat::Human => human(&doc, &args.log.display().to_string(), &mut out),
    };
    written.map_err(|e| Error::io_at(&args.log, e))?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = concat!(
        r#"{"type":"file","path":"a.go","rules":[0],"diff":"--- a/a.go\n+++ b/a.go\n@@ -1 +1 @@\n-import \"x\"\n+import \"y\"\n"}"#,
        "\n",
        r#"{"type":"file","path":"b.go","rules":[0,1],"diff":"--- a/b.go\n+++ b/b.go\n@@ -1,2 +1,3 @@\n-a\n-b\n+c\n+d\n+e\n"}"#,
        "\n",
        r#"{"type":"summary","summary":{},"rules":[{"i":0,"rule":"go::import \"x\" -> \"y\"","files":2},{"i":1,"rule":"go::call \"f\" -> replace_call \"g($args)\"","files":1},{"i":2,"rule":"dead","files":0}]}"#,
        "\n"
    );

    fn args(rule: Option<usize>, page: usize) -> ReviewArgs {
        ReviewArgs {
            log: "run.ndjson".into(),
            rule,
            page,
            per_page: 1,
            top: 5,
            format: ReviewFormat::Json,
        }
    }

    #[test]
    fn groups_files_and_lines_by_rule() {
        let doc = review(&parse(LOG).unwrap(), &args(None, 1)).unwrap();
        assert_eq!(
            (
                doc["files"].as_u64(),
                doc["added"].as_u64(),
                doc["removed"].as_u64()
            ),
            (Some(2), Some(4), Some(3))
        );
        let rule = &doc["rules"][0];
        assert_eq!(rule["files"], 2);
        // Most edited first.
        assert_eq!(rule["top"][0]["path"], "b.go");
        assert_eq!(rule["histogram"], json!({"1-2": 1, "3-5": 1}));
        assert_eq!(rule["shared"], 1, "b.go was changed by two rules");
        assert_eq!(doc["rules"][2]["files"], 0);
    }

    #[test]
    fn pages_through_one_rules_diffs() {
        let log = parse(LOG).unwrap();
        let doc = review(&log, &args(Some(1), 2)).unwrap();
        assert_eq!(doc["page"]["pages"], 2);
        assert_eq!(doc["page"]["files"][0]["path"], "b.go");
        let e = review(&log, &args(Some(9), 1)).unwrap_err().to_string();
        assert!(e.contains("rules 1 to 3"), "{e}");
        let e = review(&log, &args(Some(1), 3)).unwrap_err().to_string();
        assert!(e.contains("2 page(s)"), "{e}");
    }

    #[test]
    fn a_log_without_diffs_says_how_to_record_one() {
        let log = parse(&LOG.replace(r#","diff":"#, r#","ignored":"#)).unwrap();
        let doc = review(&log, &args(None, 1)).unwrap();
        assert!(
            doc["warnings"][0]
                .as_str()
                .unwrap()
                .contains("--detail diff")
        );
        assert!(doc["rules"][0].get("top").is_none());
        assert!(
            parse("{\"type\":\"file\"}\n")
                .unwrap_err()
                .to_string()
                .contains("no summary")
        );
    }
}
