//! Crate-wide error and result types.
//!
//! Every fallible operation in colab returns [`Result<T>`], where the error
//! variants distinguish between I/O failures (with the offending path
//! attached when known), DSL parse failures, unsupported codemod operations,
//! and CLI/configuration validation errors.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// Where a parse failed and what would have been valid there.
///
/// The parser reports byte offsets; this type resolves them to a 1-based
/// line and column and captures the offending line so callers do not have
/// to re-derive either. The LSP uses the position for diagnostic ranges;
/// agents use `expected` to correct a script without a second round trip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseDetail {
    /// What went wrong, without position information.
    pub message: String,
    /// 1-based line of the offending token.
    pub line: usize,
    /// 1-based column (in characters) of the offending token.
    pub column: usize,
    /// Byte offset of the offending token.
    pub offset: usize,
    /// Tokens that would have been valid at this position, if known.
    pub expected: Vec<String>,
    /// The offending source line with a caret beneath the column.
    pub snippet: Option<String>,
}

impl ParseDetail {
    /// Build a detail from a byte offset into `source`, resolving the
    /// line/column and extracting a caret snippet.
    pub fn at_offset(
        source: &str,
        offset: usize,
        message: impl Into<String>,
        expected: Vec<String>,
    ) -> Self {
        let offset = offset.min(source.len());
        let line_start = source[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line_end = source[line_start..]
            .find('\n')
            .map(|i| line_start + i)
            .unwrap_or(source.len());

        let line = source[..line_start].matches('\n').count() + 1;
        let column = source[line_start..offset].chars().count() + 1;
        let text = &source[line_start..line_end];
        let snippet = (!text.trim().is_empty())
            .then(|| format!("{}\n{}^", text, " ".repeat(column.saturating_sub(1))));

        Self {
            message: message.into(),
            line,
            column,
            offset,
            expected,
            snippet,
        }
    }
}

impl fmt::Display for ParseDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "line {}, column {}: {}",
            self.line, self.column, self.message
        )?;
        if !self.expected.is_empty() {
            write!(f, "; expected one of: {}", self.expected.join(", "))?;
        }
        Ok(())
    }
}

/// All errors raised by the colab library and CLI.
#[derive(Debug)]
pub enum Error {
    /// An I/O failure, optionally tagged with the path that triggered it.
    Io {
        path: Option<PathBuf>,
        source: io::Error,
    },
    /// The codemod script could not be parsed.
    Parse(ParseDetail),
    /// The script asked for a namespace/operation combination that colab
    /// does not (yet) implement.
    UnsupportedOperation(String),
    /// CLI argument or runtime configuration was invalid.
    Config(String),
}

impl Error {
    /// Wrap an I/O error with the path that produced it.
    pub fn io_at(path: impl AsRef<Path>, source: io::Error) -> Self {
        Error::Io {
            path: Some(path.as_ref().to_path_buf()),
            source,
        }
    }

    /// Process exit code that maps to this error variant.
    ///
    /// Codes are stable: tools and CI rely on them. See `--help` for
    /// the full table (exit code 10 — `--check` would-have-changed —
    /// is reported by the CLI directly, not via an `Error`).
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Parse(_) => 2,
            Error::UnsupportedOperation(_) => 3,
            Error::Io { .. } => 4,
            Error::Config(_) => 1,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io {
                path: Some(p),
                source,
            } => write!(f, "I/O error at {}: {}", p.display(), source),
            Error::Io { path: None, source } => write!(f, "I/O error: {}", source),
            Error::Parse(detail) => write!(f, "parse error at {}", detail),
            Error::UnsupportedOperation(msg) => write!(f, "unsupported operation: {}", msg),
            Error::Config(msg) => write!(f, "configuration error: {}", msg),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(source: io::Error) -> Self {
        Error::Io { path: None, source }
    }
}

/// Crate-wide [`std::result::Result`] alias defaulting to [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_a_byte_offset_to_line_and_column() {
        let source = "refactor \"x\" {\n  match go::import \"a\" { replac \"b\" }\n}\n";
        let offset = source.find("replac").unwrap();
        let detail = ParseDetail::at_offset(source, offset, "Unrecognized token", vec![]);

        assert_eq!(detail.line, 2);
        assert_eq!(detail.column, 26);
        assert_eq!(detail.offset, offset);
    }

    #[test]
    fn snippet_puts_the_caret_under_the_column() {
        let source = "one\ntwo\n";
        let detail = ParseDetail::at_offset(source, 5, "bad", vec![]);
        assert_eq!(detail.snippet.as_deref(), Some("two\n ^"));
    }

    #[test]
    fn column_counts_characters_not_bytes() {
        let source = "// é é\nmatch\n";
        let offset = source.find("match").unwrap();
        let detail = ParseDetail::at_offset(source, offset, "bad", vec![]);
        assert_eq!(detail.line, 2);
        assert_eq!(detail.column, 1);
    }

    #[test]
    fn offset_past_the_end_clamps_to_the_last_line() {
        let source = "refactor \"x\" {\n";
        let detail = ParseDetail::at_offset(source, 9_999, "unexpected EOF", vec![]);
        assert_eq!(detail.offset, source.len());
        assert_eq!(detail.line, 2);
    }

    #[test]
    fn display_names_the_position_and_the_valid_tokens() {
        let detail = ParseDetail::at_offset(
            "match\n",
            0,
            "Unrecognized token `match`",
            vec!["\"refactor\"".to_string()],
        );
        let rendered = Error::Parse(detail).to_string();
        assert!(
            rendered.starts_with("parse error at line 1, column 1:"),
            "{rendered}"
        );
        assert!(rendered.contains("expected one of: \"refactor\""));
    }

    #[test]
    fn parse_errors_keep_exit_code_two() {
        let err = Error::Parse(ParseDetail::at_offset("x", 0, "bad", vec![]));
        assert_eq!(err.exit_code(), 2);
    }
}
