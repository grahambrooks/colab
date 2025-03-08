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

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    pub(crate) replace: Replace,
}

impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Config {{ replace: {} }}", self.replace)
    }
}