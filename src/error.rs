//! Crate-wide error and result types.
//!
//! Every fallible operation in colab returns [`Result<T>`], where the error
//! variants distinguish between I/O failures (with the offending path
//! attached when known), DSL parse failures, unsupported codemod operations,
//! and CLI/configuration validation errors.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// All errors raised by the colab library and CLI.
#[derive(Debug)]
pub enum Error {
    /// An I/O failure, optionally tagged with the path that triggered it.
    Io {
        path: Option<PathBuf>,
        source: io::Error,
    },
    /// The codemod script could not be parsed.
    Parse(String),
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
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io {
                path: Some(p),
                source,
            } => write!(f, "I/O error at {}: {}", p.display(), source),
            Error::Io { path: None, source } => write!(f, "I/O error: {}", source),
            Error::Parse(msg) => write!(f, "parse error: {}", msg),
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
