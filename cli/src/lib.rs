use serde::{Deserialize, Serialize};
use serde_yaml;
use std::fs;
use std::path::Path;
use tree_sitter::{Language, Parser};

extern "C" {
    fn tree_sitter_go() -> Language;
}

#[derive(Debug, Deserialize, Serialize)]
struct Config {
    replace: Replace,
}

#[derive(Debug, Deserialize, Serialize)]
struct Replace {
    #[serde(rename = "go-module")]
    go_module: GoModule,
}

#[derive(Debug, Deserialize, Serialize)]
struct GoModule {
    from: String,
    to: String,
}

pub fn read_config<P: AsRef<Path>>(path: P) -> Result<Config, Box<dyn std::error::Error>> {
    let file = fs::File::open(path)?;
    let config: Config = serde_yaml::from_reader(file)?;
    Ok(config)
}

pub fn process_directory(parser: &Parser, config: &Config, path: &Path) {
    if path.is_dir() {
        for entry in fs::read_dir(path).expect("Failed to read directory") {
            let entry = entry.expect("Failed to read directory entry");
            let path = entry.path();
            if path.is_dir() {
                process_directory(parser, config, &path);
            } else if path.extension().and_then(|s| s.to_str()) == Some("go") {
                process_file(parser, config, &path);
            }
        }
    }
}

pub fn process_file(parser: &Parser, config: &Config, path: &Path) {
    let source_code = fs::read_to_string(path).expect("Failed to read file");
    let tree = parser.parse(&source_code, None).expect("Failed to parse file");

    let mut cursor = tree.walk();
    let mut edits = Vec::new();

    loop {
        let node = cursor.node();
        if node.is_named() && node.kind() == "import_spec" {
            let module_name = node.utf8_text(source_code.as_bytes()).expect("Failed to get text");
            if module_name.contains(&config.replace.go_module.from) {
                let new_module_name = module_name.replace(&config.replace.go_module.from, &config.replace.go_module.to);
                edits.push((node.start_byte(), node.end_byte(), new_module_name));
            }
        }

        if !cursor.goto_next_sibling() {
            break;
        }
    }

    let mut new_source_code = source_code.clone();
    for (start, end, replacement) in edits.iter().rev() {
        new_source_code.replace_range(*start..*end, replacement);
    }

    fs::write(path, new_source_code).expect("Failed to write file");
}
