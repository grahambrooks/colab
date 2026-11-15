// Load a script
fn is_script(path: &str) -> bool {
    path.ends_with(".colab")
}

const LEGACY_DIR: &str = "old.codemods"; // a longer word: untouched
