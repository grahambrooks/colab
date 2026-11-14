//! Reading Markdown by structure: `colab outline` and `colab section`,
//! and the MCP tools `colab.outline` / `colab.section`.
//!
//! An agent reads far more Markdown than it writes. The outline (headings
//! with line spans and what each section holds) costs a few lines; one
//! section costs only itself. Together they replace reading a whole file
//! to find and change one part of it — and, unlike `grep '^#'`, they do
//! not mistake a `#` comment in a code fence for a heading.

use std::fs;
use std::path::{Path, PathBuf};

use clap::Parser;
use colab_core::{Error, Result};
use colab_lang_markdown::outline::{self, Heading};
use serde_json::{Value, json};

use crate::cli::io_to_error;
use crate::format::Format;

#[derive(Parser, Debug)]
pub struct OutlineArgs {
    /// Markdown files, or directories to search for them (`.gitignore`
    /// is honoured).
    #[arg(required = true)]
    paths: Vec<PathBuf>,

    /// Only headings at this level or above (`2`: `#` and `##`).
    #[arg(long, value_name = "N")]
    depth: Option<u8>,

    /// `human` (indented headings with line spans) or `json`.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    format: Format,
}

#[derive(Parser, Debug)]
pub struct SectionArgs {
    /// The Markdown file.
    file: PathBuf,

    /// The section: a heading path such as `"Parent > Child"`, or an
    /// anchor such as `"#risks"`.
    address: String,

    /// Print only what is under the heading, not the heading itself.
    #[arg(long)]
    body: bool,

    /// `human` (the section's text) or `json`.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    format: Format,
}

/// Every Markdown file under `paths`, in walk order.
fn markdown_files(paths: &[PathBuf]) -> Result<Vec<(PathBuf, String)>> {
    struct Md;
    impl colab_core::CodeTransformer for Md {
        fn is_file_relevant(&self, path: &Path) -> bool {
            matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("md" | "markdown")
            )
        }
        fn apply(&self, source: &str) -> String {
            source.to_string()
        }
    }
    let mut out = Vec::new();
    for path in paths {
        if path.is_file() {
            let text = fs::read_to_string(path).map_err(|e| Error::io_at(path, e))?;
            out.push((path.clone(), text));
            continue;
        }
        colab_core::walker::walk(&Md, path, &mut |change| {
            out.push((change.path, change.before));
            Ok(())
        })?;
    }
    Ok(out)
}

fn shown(path: &Path) -> String {
    path.strip_prefix("./")
        .unwrap_or(path)
        .display()
        .to_string()
}

/// One file's outline as JSON: `{path, lines, headings: [{level, address,
/// anchor, line, end_line, tables?, lists?, fences?}]}`.
pub fn outline_json(path: &Path, source: &str, depth: Option<u8>) -> Value {
    let headings: Vec<Value> = outline::headings(source)
        .iter()
        .filter(|h| depth.is_none_or(|d| h.level <= d))
        .map(|h| {
            let mut v = json!({
                "level": h.level, "address": h.address(), "anchor": format!("#{}", h.slug),
                "line": h.line, "end_line": h.end_line,
            });
            for (key, n) in [
                ("tables", h.tables),
                ("lists", h.lists),
                ("fences", h.fences),
            ] {
                if n > 0 {
                    v[key] = json!(n);
                }
            }
            v
        })
        .collect();
    json!({"path": shown(path), "lines": source.lines().count(), "headings": headings})
}

fn outline_human(path: &Path, source: &str, depth: Option<u8>) -> String {
    let mut out = format!("{} ({} lines)\n", shown(path), source.lines().count());
    for h in outline::headings(source)
        .iter()
        .filter(|h| depth.is_none_or(|d| h.level <= d))
    {
        let mut holds = Vec::new();
        for (name, n) in [("table", h.tables), ("list", h.lists), ("fence", h.fences)] {
            if n > 0 {
                holds.push(format!("{n} {name}{}", if n == 1 { "" } else { "s" }));
            }
        }
        let indent = "  ".repeat(h.level.saturating_sub(1) as usize);
        let marks = "#".repeat(h.level as usize);
        let text = format!("{indent}{marks} {}", h.text);
        if holds.is_empty() {
            out.push_str(&format!("{:>5}-{:<5} {text}\n", h.line, h.end_line));
        } else {
            out.push_str(&format!(
                "{:>5}-{:<5} {text:<60} {}\n",
                h.line,
                h.end_line,
                holds.join(", ")
            ));
        }
    }
    out
}

pub fn run_outline(args: OutlineArgs) -> Result<i32> {
    let files = markdown_files(&args.paths)?;
    if files.is_empty() {
        return Err(Error::Config(
            "no Markdown files (.md, .markdown) found".into(),
        ));
    }
    match args.format {
        Format::Json => {
            let value: Vec<Value> = files
                .iter()
                .map(|(p, s)| outline_json(p, s, args.depth))
                .collect();
            println!("{}", json!({ "files": value }));
        }
        _ => {
            let parts: Vec<String> = files
                .iter()
                .map(|(p, s)| outline_human(p, s, args.depth))
                .collect();
            print!("{}", parts.join("\n"));
        }
    }
    Ok(0)
}

/// The section `address` names in `source`, or the resolution error,
/// naming `path`.
fn find_section(path: &Path, source: &str, address: &str) -> Result<Heading> {
    let headings = outline::headings(source);
    outline::resolve(&headings, address)
        .cloned()
        .map_err(|e| Error::Config(format!("{}: {e}", shown(path))))
}

/// `{path, address, anchor, line, end_line, text}` for one section.
pub fn section_json(path: &Path, source: &str, address: &str, body: bool) -> Result<Value> {
    let h = find_section(path, source, address)?;
    let from = if body { h.heading_end } else { h.start };
    let text = source[from..h.end]
        .trim_start_matches(['\n', '\r'])
        .trim_end();
    Ok(json!({
        "path": shown(path), "address": h.address(), "anchor": format!("#{}", h.slug),
        "line": h.line, "end_line": h.end_line,
        "text": format!("{text}\n"),
    }))
}

pub fn run_section(args: SectionArgs) -> Result<i32> {
    let source = fs::read_to_string(&args.file).map_err(|e| Error::io_at(&args.file, e))?;
    let value = section_json(&args.file, &source, &args.address, args.body)?;
    match args.format {
        Format::Json => println!("{value}"),
        _ => print!("{}", value["text"].as_str().unwrap_or_default()),
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// MCP
// ---------------------------------------------------------------------------

/// `colab.outline` and `colab.section`, registered by the binary (as the
/// language-server tools are), since `colab-mcp` depends on no backend.
pub struct MarkdownTools;

impl colab_mcp::ToolProvider for MarkdownTools {
    fn tools(&self) -> Vec<Value> {
        vec![
            json!({
                "name": "colab.outline",
                "description": "The heading outline of Markdown files: each heading's level, its address for colab.section and markdown::section rules (`Parent > Child`), its anchor, its line span, and how many tables, lists and code fences it holds. Read this instead of a whole file to find the part you need; a `#` line inside a code fence is not reported as a heading. Read-only.",
                "annotations": {"readOnlyHint": true},
                "inputSchema": {"type": "object", "properties": {
                    "paths": {"type": "array", "items": {"type": "string"}, "minItems": 1, "description": "Markdown files or directories, relative to `cwd`."},
                    "depth": {"type": "integer", "minimum": 1, "maximum": 6, "description": "Only headings at this level or above."},
                    "cwd": {"type": "string", "description": "Directory relative paths resolve against."}
                }, "required": ["paths"], "additionalProperties": false}
            }),
            json!({
                "name": "colab.section",
                "description": "The text of one Markdown section — its heading and everything under it, subsections included — by heading path (`Parent > Child`, outer segments need not be direct parents) or anchor (`#slug`). An ambiguous or missing address is an error that names the candidates. Read-only.",
                "annotations": {"readOnlyHint": true},
                "inputSchema": {"type": "object", "properties": {
                    "path": {"type": "string", "description": "The Markdown file, relative to `cwd`."},
                    "section": {"type": "string", "description": "Heading path or `#anchor`."},
                    "body": {"type": "boolean", "default": false, "description": "Only what is under the heading."},
                    "cwd": {"type": "string"}
                }, "required": ["path", "section"], "additionalProperties": false}
            }),
        ]
    }

    fn call(&self, name: &str, args: &Value) -> Option<Result<Value>> {
        match name {
            "colab.outline" => Some(mcp_outline(args)),
            "colab.section" => Some(mcp_section(args)),
            _ => None,
        }
    }
}

fn cwd_of(args: &Value) -> Result<PathBuf> {
    match args.get("cwd").and_then(Value::as_str) {
        Some(dir) => Ok(PathBuf::from(dir)),
        None => std::env::current_dir().map_err(io_to_error),
    }
}

fn mcp_outline(args: &Value) -> Result<Value> {
    let cwd = cwd_of(args)?;
    let paths: Vec<PathBuf> = args
        .get("paths")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Config("`paths` must be an array of strings".into()))?
        .iter()
        .filter_map(Value::as_str)
        .map(|p| cwd.join(p))
        .collect();
    let depth = args.get("depth").and_then(Value::as_u64).map(|d| d as u8);
    let files: Vec<Value> = markdown_files(&paths)?
        .iter()
        .map(|(p, s)| outline_json(p.strip_prefix(&cwd).unwrap_or(p), s, depth))
        .collect();
    Ok(json!({ "files": files }))
}

fn mcp_section(args: &Value) -> Result<Value> {
    let cwd = cwd_of(args)?;
    let rel = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Config("`path` is required".into()))?;
    let address = args
        .get("section")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Config("`section` is required".into()))?;
    let file = cwd.join(rel);
    let source = fs::read_to_string(&file).map_err(|e| Error::io_at(&file, e))?;
    let body = args.get("body").and_then(Value::as_bool).unwrap_or(false);
    section_json(Path::new(rel), &source, address, body)
}

/// Every tool the binary adds to `colab mcp`.
pub struct BinaryTools;

impl colab_mcp::ToolProvider for BinaryTools {
    fn tools(&self) -> Vec<Value> {
        let mut tools = crate::lsp_commands::LspTools.tools();
        tools.extend(MarkdownTools.tools());
        tools
    }

    fn call(&self, name: &str, args: &Value) -> Option<Result<Value>> {
        crate::lsp_commands::LspTools
            .call(name, args)
            .or_else(|| MarkdownTools.call(name, args))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "# Plan\n\n## Order\n\n| a | b |\n| - | - |\n\n```sh\n# not a heading\n```\n\n## Risks\n\nrisky\n";

    #[test]
    fn outline_lists_headings_with_spans_and_contents() {
        let v = outline_json(Path::new("plan.md"), DOC, None);
        let hs = v["headings"].as_array().unwrap();
        assert_eq!(hs.len(), 3, "{v}");
        assert_eq!(hs[1]["address"], "Plan > Order");
        assert_eq!(hs[1]["tables"], 1);
        assert_eq!(hs[1]["fences"], 1);
        assert_eq!(
            (hs[1]["line"].as_u64(), hs[1]["end_line"].as_u64()),
            (Some(3), Some(10))
        );
        assert_eq!(
            outline_json(Path::new("p"), DOC, Some(1))["headings"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let human = outline_human(Path::new("plan.md"), DOC, None);
        assert!(human.contains("    3-10      ## Order"), "{human}");
    }

    #[test]
    fn section_returns_one_section_or_names_candidates() {
        let v = section_json(Path::new("p.md"), DOC, "#risks", false).unwrap();
        assert_eq!(v["text"], "## Risks\n\nrisky\n");
        let body = section_json(Path::new("p.md"), DOC, "Risks", true).unwrap();
        assert_eq!(body["text"], "risky\n");
        let err = section_json(Path::new("p.md"), DOC, "Risk", false).unwrap_err();
        assert!(
            err.to_string().contains("did you mean `Plan > Risks`"),
            "{err}"
        );
    }
}
