//! MCP resources: colab's own documentation, for an agent to read without
//! leaving the server (`resources/list`, `resources/read`), and
//! `llms-full.txt`, the same documents as one text, at the repository's
//! root for anything that reads that convention.
//!
//! The documents are compiled in, so what the server says is what this
//! build does; `llms-full.txt` is generated from them, and a test keeps
//! the committed copy current (`COLAB_BLESS=1` rewrites it).
use std::fmt::Write as _;

use serde_json::{Value, json};

/// One document: its URI, title, what it is for, and its text.
pub struct Doc {
    pub uri: &'static str,
    pub path: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub text: &'static str,
}

pub const DOCS: &[Doc] = &[
    Doc {
        uri: "colab://docs/dsl.md",
        path: "docs/dsl.md",
        name: "The codemod DSL",
        description: "Every rule form, action, constraint (`where`, `let`, `in`, `type`, `resolves`), named patterns, rewriters, scans and stages.",
        text: include_str!("../../../docs/dsl.md"),
    },
    Doc {
        uri: "colab://docs/cli.md",
        path: "docs/cli.md",
        name: "The CLI and MCP tools",
        description: "Every command and flag, output formats, exit codes, and the MCP tool list.",
        text: include_str!("../../../docs/cli.md"),
    },
    Doc {
        uri: "colab://docs/features.md",
        path: "docs/features.md",
        name: "Per-backend features and caveats",
        description: "What each of the 15 language and config backends supports, and where each falls short.",
        text: include_str!("../../../docs/features.md"),
    },
    Doc {
        uri: "colab://docs/operations.md",
        path: "docs/operations.md",
        name: "Installing and operating colab",
        description: "Install, agents (plugin, agent install / uninstall, the post-edit hook), colab.toml and CI, performance, the evaluation results, caches, upgrading, troubleshooting.",
        text: include_str!("../../../docs/operations.md"),
    },
    Doc {
        uri: "colab://README.md",
        path: "README.md",
        name: "What colab is",
        description: "Overview, install, a first script, and where to read next.",
        text: include_str!("../../../README.md"),
    },
];

/// Every document as one text, `llms-full.txt`.
pub fn llms_full() -> String {
    let mut out = String::from(
        "# colab\n\n> colab, the structural codemod tool: repo-wide structural edits across 13 source languages, TOML / JSON / Markdown and any text file, from a small DSL, a CLI, an MCP server and an LSP. This file is generated from the documents below (`cargo test -p colab-mcp`, `COLAB_BLESS=1` to refresh).\n",
    );
    for doc in DOCS {
        let _ = write!(out, "\n\n---\n\n<!-- file: {} -->\n\n", doc.path);
        out.push_str(doc.text.trim_end());
        out.push('\n');
    }
    out
}

const LLMS_URI: &str = "colab://llms-full.txt";

/// `resources/list`.
pub fn list() -> Value {
    let mut resources: Vec<Value> = DOCS
        .iter()
        .map(|d| {
            json!({"uri": d.uri, "name": d.name, "description": d.description, "mimeType": "text/markdown"})
        })
        .collect();
    resources.push(json!({
        "uri": LLMS_URI,
        "name": "All of colab's documentation",
        "description": "The documents above as one text (llms-full.txt).",
        "mimeType": "text/plain",
    }));
    json!({"resources": resources})
}

/// `resources/read` of `uri`, or `None` when there is no such resource.
pub fn read(uri: &str) -> Option<Value> {
    let (text, mime) = if uri == LLMS_URI {
        (llms_full(), "text/plain")
    } else {
        let doc = DOCS.iter().find(|d| d.uri == uri)?;
        (doc.text.to_string(), "text/markdown")
    };
    Some(json!({"contents": [{"uri": uri, "mimeType": mime, "text": text}]}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_committed_llms_full_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../llms-full.txt");
        let want = llms_full();
        if std::env::var("COLAB_BLESS").is_ok_and(|v| v == "1") {
            std::fs::write(&path, &want).unwrap();
        }
        let have = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            have == want,
            "llms-full.txt is out of date with docs/; regenerate it with COLAB_BLESS=1 cargo test -p colab-mcp"
        );
    }

    #[test]
    fn every_listed_resource_reads() {
        for r in list()["resources"].as_array().unwrap() {
            let uri = r["uri"].as_str().unwrap();
            let read = read(uri).unwrap();
            assert!(
                !read["contents"][0]["text"].as_str().unwrap().is_empty(),
                "{uri}"
            );
        }
        assert!(read("colab://nope").is_none());
    }
}
