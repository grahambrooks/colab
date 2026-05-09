//! Static descriptors for the four MCP tools colab exposes. Used by
//! `tools/list` so an agent can discover them without trial calls.

use serde_json::{Value, json};

const SCRIPT_PARAM: &str = "Codemod script source (the contents of a `.codemod` file).";
const PATHS_PARAM: &str = "Files or directories to walk recursively. Each entry is processed in order.";

pub fn list() -> Vec<Value> {
    vec![
        json!({
            "name": "colab.schema",
            "description": "Return the JSON capability schema for every registered backend (matches `colab schema`).",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "colab.lint_script",
            "description": "Parse + compile a codemod script without running it. Returns `{ok: true, name, rule_count}` on success or `{ok: false, error, exit_code}` on failure.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "script": { "type": "string", "description": SCRIPT_PARAM }
                },
                "required": ["script"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "colab.preview",
            "description": "Apply a script to one or more paths in dry-run mode. Returns one entry per file with a unified diff. Disk is not modified.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "script": { "type": "string", "description": SCRIPT_PARAM },
                    "paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 1,
                        "description": PATHS_PARAM
                    }
                },
                "required": ["script", "paths"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "colab.apply",
            "description": "Apply a script to one or more paths and write changes back to disk. Returns one entry per file with a unified diff of what was applied. Use colab.preview first to verify.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "script": { "type": "string", "description": SCRIPT_PARAM },
                    "paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 1,
                        "description": PATHS_PARAM
                    }
                },
                "required": ["script", "paths"],
                "additionalProperties": false
            }
        }),
    ]
}
