use crate::codemod::go;
use crate::refactor::CodeTransformer;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Replace {
    #[serde(rename = "go-module")]
    pub(crate) go_module: GoModule,
}

impl fmt::Display for Replace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Replace {{ go_module: {} }}", self.go_module)
    }
}

impl CodeTransformer for Replace {
    fn is_file_relevant(&self, path: &std::path::Path) -> bool {
        self.go_module.is_file_relevant(path)
    }
    fn apply(&self, source_code: &String) -> String {
        self.go_module.apply(source_code)
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct GoModule {
    pub(crate) from: String,
    pub(crate) to: String,
}

impl fmt::Display for GoModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GoModule {{ from: {}, to: {} }}", self.from, self.to)
    }
}

impl CodeTransformer for GoModule {
    fn is_file_relevant(&self, path: &std::path::Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("go")
    }
    fn apply(&self, source_code: &String) -> String {
        go::imports::rename(self, source_code)
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Refactoring {
    pub(crate) replace: Replace,
}

impl fmt::Display for Refactoring {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Refactoring {{ replace: {} }}", self.replace)
    }
}

impl CodeTransformer for Refactoring {
    fn is_file_relevant(&self, path: &std::path::Path) -> bool {
        self.replace.is_file_relevant(path)
    }
    fn apply(&self, source_code: &String) -> String {
        self.replace.apply(source_code)
    }
}
