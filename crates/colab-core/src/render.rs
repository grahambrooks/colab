//! Turning a [`RunReport`] into output an agent can afford to read.
//!
//! Both output surfaces — `colab refactor`'s JSON reporter and the
//! `colab-mcp` tool responses — render through this module, so the two
//! cannot drift.
//!
//! The shape is bounded, and every level includes the counters:
//!
//! ```json
//! {"summary":{"visited":412,"scanned":38,"changed":3,"skipped":0,"elapsed_ms":84},
//!  "rules":[{"i":0,"rule":"go::import \"a\" -> \"b\"","files":3},
//!           {"i":1,"rule":"go::symbol \"X\" -> \"Y\"","files":0}],
//!  "changed":["cmd/main.go"],
//!  "diffs":[{"path":"cmd/main.go","diff":"@@ …"}]}
//! ```
//!
//! Files that were scanned but unchanged never appear — they survive only
//! as the `scanned`/`changed` counters. A rule with `"files": 0` is dead:
//! it compiled and ran but never changed a byte, which is nearly always a
//! mistake in the script.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::fileops::FileOp;
use crate::report::RunReport;
use crate::walker::FileChange;

/// How much of the run to render.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Detail {
    /// Aggregate counters only. The cheapest useful answer.
    Summary,
    /// Aggregate counters, per-rule match counts, and changed paths.
    /// Enough to judge blast radius without reading a diff.
    #[default]
    Counts,
    /// Everything in `Counts` plus per-file unified diffs, capped.
    Diff,
}

impl Detail {
    /// Parse the wire form used by `--detail` and the MCP `detail`
    /// argument. Returns `None` for anything unrecognized so the caller
    /// can report a precise error.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "summary" => Some(Detail::Summary),
            "counts" => Some(Detail::Counts),
            "diff" => Some(Detail::Diff),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Detail::Summary => "summary",
            Detail::Counts => "counts",
            Detail::Diff => "diff",
        }
    }

    /// The values accepted by [`parse`](Self::parse), for error messages.
    pub const NAMES: [&'static str; 3] = ["summary", "counts", "diff"];
}

/// Default cap on how many per-file diffs are rendered.
pub const DEFAULT_MAX_FILES: usize = 20;
/// Default cap on the size of any single rendered diff.
pub const DEFAULT_MAX_DIFF_BYTES: usize = 2000;

/// Rendering limits. Both caps apply only to `Detail::Diff`; the
/// `changed` path list is never truncated, since it is one short string
/// per changed file and is what a caller needs to act on.
#[derive(Clone, Copy, Debug)]
pub struct RenderOptions {
    pub detail: Detail,
    pub max_files: usize,
    pub max_diff_bytes: usize,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            detail: Detail::default(),
            max_files: DEFAULT_MAX_FILES,
            max_diff_bytes: DEFAULT_MAX_DIFF_BYTES,
        }
    }
}

/// Accumulates the per-file half of a run's output within the configured
/// caps, so a repo-wide apply cannot produce an unbounded response.
///
/// Only changed files are retained, and diffs are rendered eagerly (and
/// capped) as files arrive rather than holding every `before`/`after`
/// pair until the end.
#[derive(Debug)]
pub struct ChangeCollector {
    options: RenderOptions,
    changed: Vec<PathBuf>,
    diffs: Vec<(PathBuf, String)>,
    diffs_omitted: u64,
    bytes_omitted: u64,
}

impl ChangeCollector {
    pub fn new(options: RenderOptions) -> Self {
        Self {
            options,
            changed: Vec::new(),
            diffs: Vec::new(),
            diffs_omitted: 0,
            bytes_omitted: 0,
        }
    }

    pub fn options(&self) -> RenderOptions {
        self.options
    }

    /// Record one processed file. Unchanged files are dropped.
    pub fn push(&mut self, change: &FileChange) {
        if !change.changed() {
            return;
        }
        self.changed.push(change.path.clone());

        if self.options.detail != Detail::Diff {
            return;
        }
        if self.diffs.len() >= self.options.max_files {
            self.diffs_omitted += 1;
            return;
        }

        let full = unified_diff(&change.path, &change.before, &change.after);
        let (text, dropped) = truncate_utf8(full, self.options.max_diff_bytes);
        self.bytes_omitted += dropped as u64;
        self.diffs.push((change.path.clone(), text));
    }

    /// Paths of every changed file, in the order they were processed.
    pub fn changed(&self) -> &[PathBuf] {
        &self.changed
    }

    /// Render the whole run as one JSON document.
    pub fn to_json(&self, report: &RunReport) -> Value {
        let mut root = json!({ "summary": summary_json(report) });
        let obj = root.as_object_mut().expect("summary_json builds an object");

        if self.options.detail == Detail::Summary {
            return root;
        }

        obj.insert("rules".to_string(), rules_json(report));
        obj.insert(
            "changed".to_string(),
            Value::Array(
                self.changed
                    .iter()
                    .map(|p| Value::String(display_path(p)))
                    .collect(),
            ),
        );

        file_ops_json(report, obj);

        if !report.skipped.is_empty() {
            obj.insert(
                "skipped".to_string(),
                Value::Array(
                    report
                        .skipped
                        .iter()
                        .map(|s| json!({"path": display_path(&s.path), "reason": s.reason}))
                        .collect(),
                ),
            );
        }

        if self.options.detail == Detail::Diff {
            obj.insert(
                "diffs".to_string(),
                Value::Array(
                    self.diffs
                        .iter()
                        .map(|(path, diff)| json!({"path": display_path(path), "diff": diff}))
                        .collect(),
                ),
            );
            if self.diffs_omitted > 0 || self.bytes_omitted > 0 {
                obj.insert(
                    "truncated".to_string(),
                    json!({"files": self.diffs_omitted, "bytes": self.bytes_omitted}),
                );
            }
        }

        root
    }
}

/// The aggregate counters. Present at every detail level. `moved`,
/// `created`, and `deleted` appear only when a run has file operations,
/// so ordinary codemod output keeps its shape.
pub fn summary_json(report: &RunReport) -> Value {
    let mut summary = json!({
        "visited": report.files_visited,
        "scanned": report.files_scanned,
        "changed": report.files_changed,
        "skipped": report.skipped.len(),
        "bytes_before": report.bytes_before,
        "bytes_after": report.bytes_after,
        "elapsed_ms": report.elapsed.as_millis() as u64,
    });
    if !report.file_ops.is_empty() {
        let count =
            |want: fn(&FileOp) -> bool| report.file_ops.iter().filter(|op| want(op)).count();
        summary["moved"] = json!(count(|op| matches!(op, FileOp::Move { .. })));
        summary["created"] = json!(count(|op| matches!(op, FileOp::Create { .. })));
        summary["deleted"] = json!(count(|op| matches!(op, FileOp::Delete { .. })));
    }
    summary
}

/// `moved: [{from, to}]`, `created: [path]`, `deleted: [path]`, each only
/// when non-empty.
fn file_ops_json(report: &RunReport, obj: &mut serde_json::Map<String, Value>) {
    let mut moved = Vec::new();
    let mut created = Vec::new();
    let mut deleted = Vec::new();
    for op in &report.file_ops {
        match op {
            FileOp::Move { from, to } => {
                moved.push(json!({"from": display_path(from), "to": display_path(to)}))
            }
            FileOp::Create { path } => created.push(json!(display_path(path))),
            FileOp::Delete { path, .. } => deleted.push(json!(display_path(path))),
        }
    }
    for (key, list) in [("moved", moved), ("created", created), ("deleted", deleted)] {
        if !list.is_empty() {
            obj.insert(key.to_string(), Value::Array(list));
        }
    }
}

/// The git-style header announcing a file operation in `--format diff`
/// output, so the stream stays `git apply` input: renames carry
/// `rename from` / `rename to`, creates and deletes their file mode.
/// `pure` marks a move with no content change (`similarity index 100%`),
/// which `git apply` needs to accept a rename with no hunks.
pub fn write_file_op_header<W: Write>(out: &mut W, op: &FileOp, pure: bool) -> io::Result<()> {
    match op {
        FileOp::Move { from, to } => {
            writeln!(out, "diff --git a/{} b/{}", from.display(), to.display())?;
            if pure {
                writeln!(out, "similarity index 100%")?;
            }
            writeln!(
                out,
                "rename from {}\nrename to {}",
                from.display(),
                to.display()
            )
        }
        FileOp::Create { path } => writeln!(
            out,
            "diff --git a/{p} b/{p}\nnew file mode 100644",
            p = path.display()
        ),
        FileOp::Delete { path, before } => {
            writeln!(
                out,
                "diff --git a/{p} b/{p}\ndeleted file mode 100644",
                p = path.display()
            )?;
            if before.is_empty() {
                return Ok(());
            }
            let diff = similar::TextDiff::from_lines(before.as_str(), "");
            write!(
                out,
                "{}",
                diff.unified_diff()
                    .header(&format!("a/{}", path.display()), "/dev/null")
            )
        }
    }
}

/// A unified diff whose old and new sides have different paths — a file
/// that was moved and edited. A created file's old side is `/dev/null`.
pub fn write_unified_diff_between<W: Write>(
    out: &mut W,
    from: Option<&Path>,
    to: &Path,
    before: &str,
    after: &str,
) -> io::Result<()> {
    let header_a = match from {
        Some(from) => format!("a/{}", from.display()),
        None => "/dev/null".to_string(),
    };
    let header_b = format!("b/{}", to.display());
    let diff = similar::TextDiff::from_lines(before, after);
    write!(out, "{}", diff.unified_diff().header(&header_a, &header_b))
}

/// Per-rule match counts. `files: 0` marks a dead rule.
pub fn rules_json(report: &RunReport) -> Value {
    Value::Array(
        report
            .rules
            .iter()
            .map(|r| {
                let mut rule = json!({"i": r.index, "rule": r.rule, "files": r.files_matched});
                if !r.near_misses.is_empty() {
                    rule["near"] = Value::Array(
                        r.near_misses
                            .iter()
                            .map(|m| json!({"value": m.value, "files": m.files}))
                            .collect(),
                    );
                }
                rule
            })
            .collect(),
    )
}

/// Warnings that apply to any run: rules that matched nothing, and an
/// explanation of *why* nothing changed when nothing did.
///
/// Rules are identified by their `Display` text rather than a number,
/// because the two surfaces number them differently — the JSON `i` field
/// is a 0-based array index, while the human format lists them 1-based —
/// and a warning that disagrees with the data beside it is worse than no
/// number at all.
pub fn advisories(report: &RunReport) -> Vec<String> {
    let mut out: Vec<String> = report.warnings.clone();
    out.extend(report.dead_rules().map(|r| {
        let mut line = format!("matched no files: {}", r.rule);
        if !r.near_misses.is_empty() {
            let seen: Vec<String> = r
                .near_misses
                .iter()
                .map(|m| format!("`{}` ({} file(s))", m.value, m.files))
                .collect();
            line.push_str(&format!("; nearest seen: {}", seen.join(", ")));
        }
        line
    }));
    if let Some(reason) = explain_no_changes(report) {
        out.push(reason);
    }
    out
}

/// One-line human explanation of a run that changed nothing, or `None`
/// when something did change.
///
/// This is the counterpart to the three-counter split: it turns an
/// otherwise ambiguous empty result into a specific next step.
pub fn explain_no_changes(report: &RunReport) -> Option<String> {
    if report.files_changed > 0 || !report.file_ops.is_empty() {
        return None;
    }
    if report.files_visited == 0 {
        return Some(
            "No files were visited — check the paths, --include/--exclude globs, \
             and whether .gitignore is excluding the tree (--no-ignore overrides)."
                .to_string(),
        );
    }
    if report.files_scanned == 0 {
        return Some(format!(
            "Visited {} file(s) but none belong to a language this script targets.",
            report.files_visited
        ));
    }
    Some(format!(
        "Scanned {} file(s); no rule matched.",
        report.files_scanned
    ))
}

/// Render a unified diff for one file pair as a string.
pub fn unified_diff(path: &Path, before: &str, after: &str) -> String {
    let mut out = Vec::new();
    // Writing to a Vec cannot fail.
    write_unified_diff(&mut out, path, before, after).expect("writing a diff to a Vec cannot fail");
    String::from_utf8_lossy(&out).into_owned()
}

/// Write a unified diff for one file pair to `out`.
pub fn write_unified_diff<W: Write>(
    out: &mut W,
    path: &Path,
    before: &str,
    after: &str,
) -> io::Result<()> {
    let display = path.display();
    let header_a = format!("a/{}", display);
    let header_b = format!("b/{}", display);
    let diff = similar::TextDiff::from_lines(before, after);
    write!(out, "{}", diff.unified_diff().header(&header_a, &header_b))
}

/// Truncate `text` to at most `limit` bytes on a char boundary,
/// returning the (possibly shortened) text and how many bytes were
/// dropped. A truncated diff gains a trailing marker so a reader never
/// mistakes it for the whole hunk set.
fn truncate_utf8(text: String, limit: usize) -> (String, usize) {
    if text.len() <= limit {
        return (text, 0);
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let dropped = text.len() - end;
    let mut truncated = text[..end].to_string();
    truncated.push_str(&format!("\n… {} more bytes truncated\n", dropped));
    (truncated, dropped)
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

// ---------------------------------------------------------------------------
// Search results (`colab find` / `colab.find`)
// ---------------------------------------------------------------------------

/// The longest match text reported at `detailed`; concise output shows
/// only the first line.
pub const MAX_MATCH_TEXT: usize = 2000;

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("");
    if line.len() < text.trim_end().len() {
        format!("{line} …")
    } else {
        line.to_string()
    }
}

fn capped(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// One JSON document for a search: hits (capped), counters, per-rule
/// totals with near misses for rules that found nothing, and warnings.
pub fn find_json(report: &crate::find::FindReport) -> Value {
    use crate::find::FindDetail;
    let detailed = report.detail == FindDetail::Detailed;
    let matches: Vec<Value> = report
        .hits
        .iter()
        .map(|hit| {
            let mut m = json!({
                "path": hit.path.display().to_string(),
                "line": hit.located.line,
                "col": hit.located.column,
                "rule": hit.rule,
            });
            if detailed {
                m["text"] = json!(capped(&hit.located.text, MAX_MATCH_TEXT));
                if !hit.located.captures.is_empty() {
                    let captures: serde_json::Map<String, Value> = hit
                        .located
                        .captures
                        .iter()
                        .map(|(k, v)| (k.clone(), json!(capped(v, MAX_MATCH_TEXT))))
                        .collect();
                    m["captures"] = Value::Object(captures);
                }
            } else {
                m["text"] = json!(first_line(&hit.located.text));
            }
            m
        })
        .collect();
    let rules: Vec<Value> = report
        .run
        .rules
        .iter()
        .map(|r| {
            let mut rule = json!({
                "i": r.index,
                "rule": r.rule,
                "files": r.files_matched,
                "matches": report.matches_per_rule[r.index],
            });
            if !r.near_misses.is_empty() {
                rule["near"] = Value::Array(
                    r.near_misses
                        .iter()
                        .map(|m| json!({"value": m.value, "files": m.files}))
                        .collect(),
                );
            }
            rule
        })
        .collect();
    let mut out = json!({
        "matches": matches,
        "summary": {
            "visited": report.run.files_visited,
            "scanned": report.run.files_scanned,
            "files": report.run.files_changed,
            "matches": report.total,
        },
        "rules": rules,
    });
    if report.truncated() > 0 {
        out["truncated"] = json!(report.truncated());
    }
    let warnings = find_warnings(report);
    if !warnings.is_empty() {
        out["warnings"] = json!(warnings);
    }
    out
}

/// Dead-rule warnings phrased for a search rather than a rewrite.
pub fn find_warnings(report: &crate::find::FindReport) -> Vec<String> {
    let mut out: Vec<String> = advisories(&report.run)
        .into_iter()
        .filter(|w| w.starts_with("matched no files"))
        .map(|w| w.replacen("matched no files", "found nothing for", 1))
        .collect();
    if report.total == 0 {
        if report.run.files_visited == 0 {
            out.push(
                "No files were visited — check the paths, --include/--exclude globs, \
                 and whether .gitignore is excluding the tree (--no-ignore overrides)."
                    .into(),
            );
        } else if report.run.files_scanned == 0 {
            out.push(format!(
                "Visited {} file(s) but none belong to a language this script targets.",
                report.run.files_visited
            ));
        }
    }
    out
}

/// `path:line:col: text` per hit — grep's shape, so editors and agents
/// already know how to read it — then a one-line total and warnings.
pub fn write_find_human<W: Write>(out: &mut W, report: &crate::find::FindReport) -> io::Result<()> {
    use crate::find::FindDetail;
    let multi_rule = report.run.rules.len() > 1;
    for hit in &report.hits {
        let tag = if multi_rule {
            format!(" [rule {}]", hit.rule + 1)
        } else {
            String::new()
        };
        writeln!(
            out,
            "{}:{}:{}:{} {}",
            hit.path.display(),
            hit.located.line,
            hit.located.column,
            tag,
            first_line(&hit.located.text)
        )?;
        if report.detail == FindDetail::Detailed {
            for (name, value) in &hit.located.captures {
                writeln!(out, "    ${name} = {}", first_line(value))?;
            }
        }
    }
    write!(
        out,
        "{} match(es) in {} file(s); {} scanned",
        report.total, report.run.files_changed, report.run.files_scanned
    )?;
    if report.truncated() > 0 {
        write!(
            out,
            "; {} not shown (raise --max-results)",
            report.truncated()
        )?;
    }
    writeln!(out)?;
    for warning in find_warnings(report) {
        writeln!(out, "warning: {warning}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::RunReport;

    fn change(path: &str, before: &str, after: &str) -> FileChange {
        FileChange {
            path: PathBuf::from(path),
            before: before.to_string(),
            after: after.to_string(),
            rules_fired: if before == after { vec![] } else { vec![0] },
        }
    }

    fn report_with(changed: u64) -> RunReport {
        let mut report = RunReport::with_rules(["go::import \"a\" -> \"b\"", "go::symbol dead"]);
        report.files_visited = 10;
        report.files_scanned = 4;
        report.files_changed = changed;
        report
    }

    #[test]
    fn unchanged_files_are_dropped() {
        let mut collector = ChangeCollector::new(RenderOptions::default());
        collector.push(&change("a.go", "same", "same"));
        collector.push(&change("b.go", "old", "new"));
        assert_eq!(collector.changed(), [PathBuf::from("b.go")]);
    }

    #[test]
    fn summary_detail_omits_rules_and_paths() {
        let collector = ChangeCollector::new(RenderOptions {
            detail: Detail::Summary,
            ..Default::default()
        });
        let json = collector.to_json(&report_with(1));
        assert!(json.get("summary").is_some());
        assert!(json.get("rules").is_none());
        assert!(json.get("changed").is_none());
    }

    #[test]
    fn counts_detail_reports_dead_rules_but_no_diffs() {
        let mut collector = ChangeCollector::new(RenderOptions::default());
        collector.push(&change("b.go", "old", "new"));
        let json = collector.to_json(&report_with(1));

        assert_eq!(json["rules"][0]["i"], 0);
        assert_eq!(json["rules"][1]["files"], 0);
        assert_eq!(json["changed"][0], "b.go");
        assert!(json.get("diffs").is_none());
    }

    #[test]
    fn diff_detail_caps_the_file_count_and_reports_the_residue() {
        let mut collector = ChangeCollector::new(RenderOptions {
            detail: Detail::Diff,
            max_files: 1,
            ..Default::default()
        });
        collector.push(&change("a.go", "one\n", "ONE\n"));
        collector.push(&change("b.go", "two\n", "TWO\n"));

        let json = collector.to_json(&report_with(2));
        assert_eq!(json["diffs"].as_array().unwrap().len(), 1);
        assert_eq!(json["truncated"]["files"], 1);
        // Both paths are still listed — only the diffs are capped.
        assert_eq!(json["changed"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn oversized_diffs_are_truncated_on_a_char_boundary() {
        let before = "é\n".repeat(400);
        let after = "e\n".repeat(400);
        let mut collector = ChangeCollector::new(RenderOptions {
            detail: Detail::Diff,
            max_diff_bytes: 64,
            ..Default::default()
        });
        collector.push(&change("a.go", &before, &after));

        let json = collector.to_json(&report_with(1));
        let diff = json["diffs"][0]["diff"].as_str().unwrap();
        assert!(diff.contains("more bytes truncated"), "got: {diff}");
        assert!(json["truncated"]["bytes"].as_u64().unwrap() > 0);
    }

    #[test]
    fn skipped_files_appear_only_when_present() {
        let mut report = report_with(0);
        let collector = ChangeCollector::new(RenderOptions::default());
        assert!(collector.to_json(&report).get("skipped").is_none());

        report.skip("bin.py", "stream did not contain valid UTF-8");
        let json = collector.to_json(&report);
        assert_eq!(json["skipped"][0]["path"], "bin.py");
        assert_eq!(json["summary"]["skipped"], 1);
    }

    #[test]
    fn empty_results_are_explained_by_which_counter_is_zero() {
        let mut report = RunReport::default();
        assert!(explain_no_changes(&report).unwrap().contains("No files"));

        report.files_visited = 400;
        assert!(
            explain_no_changes(&report)
                .unwrap()
                .contains("none belong to a language")
        );

        report.files_scanned = 38;
        assert!(
            explain_no_changes(&report)
                .unwrap()
                .contains("no rule matched")
        );

        report.files_changed = 1;
        assert!(explain_no_changes(&report).is_none());
    }

    #[test]
    fn detail_round_trips_through_its_wire_form() {
        for name in Detail::NAMES {
            assert_eq!(Detail::parse(name).unwrap().as_str(), name);
        }
        assert!(Detail::parse("verbose").is_none());
    }
}
