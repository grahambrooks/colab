const CAPABILITIES: &[Capability] = &[
    colab_rewrite::language::pattern_capability("Match code by shape."),
];

pub struct Options {
    pub name: String,
}

fn build(target: String, replacement: String) -> Result<Box<dyn Operation>> {
    let op = crate::LANG.symbol_rename(target, replacement);
    drop(op);
    Ok(Box::new(crate::LANG.symbol_rename(target, replacement)))
}

fn options() -> Options {
    Options {
        name: String::new(),
    }
}
