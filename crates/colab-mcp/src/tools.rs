//! Static descriptors for the MCP tools colab exposes. Used by
//! `tools/list` so an agent can discover them without trial calls.
//!
//! Discovery is deliberately split into three tools rather than one:
//! `colab.schema` for one language is a few hundred bytes, while the
//! whole registry is several kilobytes an agent rarely needs.

use colab_core::render::{DEFAULT_MAX_DIFF_BYTES, DEFAULT_MAX_FILES};
use serde_json::{Value, json};

const SCRIPT_PARAM: &str = "Codemod script source (the contents of a `.codemod` file).";
const PATHS_PARAM: &str = "Files or directories to walk recursively. Each entry is processed in order. \
     Relative paths resolve against `cwd` when given, otherwise the server's \
     working directory.";
const CWD_PARAM: &str = "Directory that relative `paths` resolve against, and the base for `include \"...\"` \
     directives in the script. Strongly recommended: without it, relative paths depend on \
     wherever the server process was started.";
const DETAIL_PARAM: &str = "How much to return. `summary`: counters only (cheapest). \
     `counts` (default): counters, per-rule match counts, and changed paths — enough to \
     judge blast radius. `diff`: adds capped unified diffs. A rule reported with \
     `files: 0` matched nothing and is almost certainly wrong.";

fn run_properties() -> Value {
    json!({
        "script": { "type": "string", "description": SCRIPT_PARAM },
        "paths": {
            "type": "array",
            "items": { "type": "string" },
            "minItems": 1,
            "description": PATHS_PARAM
        },
        "cwd": { "type": "string", "description": CWD_PARAM },
        "detail": {
            "type": "string",
            "enum": ["summary", "counts", "diff"],
            "default": "counts",
            "description": DETAIL_PARAM
        },
        "max_files": {
            "type": "integer",
            "minimum": 0,
            "default": DEFAULT_MAX_FILES,
            "description": "At `detail: \"diff\"`, the maximum number of per-file diffs to return."
        },
        "max_diff_bytes": {
            "type": "integer",
            "minimum": 0,
            "default": DEFAULT_MAX_DIFF_BYTES,
            "description": "At `detail: \"diff\"`, the maximum size of any single diff."
        }
    })
}

pub fn list() -> Vec<Value> {
    vec![
        json!({
            "name": "colab.list_languages",
            "description": "List the registered language backends by name. Start here: it is a fraction of the size of colab.schema.",
            "annotations": { "readOnlyHint": true },
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "colab.list_rules",
            "description": "List the modules and actions one language backend supports, e.g. `go::import` with `replace` / `delete` / `ensure`.",
            "annotations": { "readOnlyHint": true },
            "inputSchema": {
                "type": "object",
                "properties": {
                    "lang": { "type": "string", "description": "Language name, e.g. `go` or `rust`." }
                },
                "required": ["lang"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "colab.schema",
            "description": "Return the full JSON capability schema (matches `colab schema`). Pass `lang` to restrict it to one backend; the unfiltered document is several KB.",
            "annotations": { "readOnlyHint": true },
            "inputSchema": {
                "type": "object",
                "properties": {
                    "lang": { "type": "string", "description": "Restrict the schema to this language." }
                },
                "required": [],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "colab.lint_script",
            "description": "Parse + compile a codemod script without running it. Returns `{ok: true, name, rule_count}`, or an error with the failing line, column, and the tokens that would have been valid.",
            "annotations": { "readOnlyHint": true },
            "inputSchema": {
                "type": "object",
                "properties": {
                    "script": { "type": "string", "description": SCRIPT_PARAM },
                    "cwd": { "type": "string", "description": CWD_PARAM }
                },
                "required": ["script"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "colab.preview",
            "description": "Apply a script to one or more paths in dry-run mode. Disk is not modified. Returns a summary, per-rule match counts, and the changed paths; pass `detail: \"diff\"` for hunks.",
            "annotations": { "readOnlyHint": true },
            "inputSchema": {
                "type": "object",
                "properties": run_properties(),
                "required": ["script", "paths"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "colab.apply",
            "description": "Apply a script to one or more paths and write the changes to disk. Run colab.preview first and check the per-rule counts.",
            "annotations": { "readOnlyHint": false, "destructiveHint": true, "idempotentHint": false },
            "inputSchema": {
                "type": "object",
                "properties": run_properties(),
                "required": ["script", "paths"],
                "additionalProperties": false
            }
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_declares_a_name_schema_and_annotations() {
        for tool in list() {
            let name = tool["name"].as_str().expect("name");
            assert!(name.starts_with("colab."), "{name}");
            assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
            assert!(
                tool["annotations"].is_object(),
                "{name} must declare annotations so a host can tell reads from writes"
            );
        }
    }

    #[test]
    fn only_apply_is_marked_destructive() {
        for tool in list() {
            let name = tool["name"].as_str().unwrap();
            let destructive = tool["annotations"]["destructiveHint"]
                .as_bool()
                .unwrap_or(false);
            assert_eq!(destructive, name == "colab.apply", "{name}");
        }
    }

    #[test]
    fn run_tools_default_to_the_counts_detail() {
        for tool in list() {
            let name = tool["name"].as_str().unwrap();
            if name == "colab.preview" || name == "colab.apply" {
                assert_eq!(
                    tool["inputSchema"]["properties"]["detail"]["default"],
                    "counts"
                );
            }
        }
    }
}
