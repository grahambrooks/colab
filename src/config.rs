use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Replace {
    #[serde(rename = "go-module")]
    pub(crate) go_module: GoModule,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct GoModule {
    pub(crate) from: String,
    pub(crate) to: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    pub(crate) replace: Replace,
}

pub fn read_config<P: AsRef<Path>>(path: P) -> Result<Config, Box<dyn std::error::Error>> {
    let file = fs::File::open(path)?;
    let config: Config = serde_yaml::from_reader(file)?;
    Ok(config)
}
