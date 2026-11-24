const CAPABILITIES: &[Capability] = &[
    Capability {
        module: "pattern",
        description: "Match code by shape.",
        actions: &[ActionCapability {
            name: "replace",
            description: "Replace each match.",
        }],
    },
];

pub struct Options {
    pub name: String,
    legacy: bool,
}

fn build(target: String, replacement: String) -> Result<Box<dyn Operation>> {
    let op = SymbolRename { from: target.into(), to: replacement.into() };
    drop(op);
    Ok(Box::new(SymbolRename { from: target.into(), to: replacement.into() }))
}

fn options() -> Options {
    Options {
        name: String::new(),
        legacy: false,
    }
}
