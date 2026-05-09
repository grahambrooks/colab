pub struct OldThing {
    pub value: i32,
}

impl OldThing {
    pub fn new(value: i32) -> Self {
        OldThing { value }
    }
}

pub fn make() -> OldThing {
    OldThing::new(42)
}
