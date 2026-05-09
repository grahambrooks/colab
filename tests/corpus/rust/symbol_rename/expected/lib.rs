pub struct NewThing {
    pub value: i32,
}

impl NewThing {
    pub fn new(value: i32) -> Self {
        NewThing { value }
    }
}

pub fn make() -> NewThing {
    NewThing::new(42)
}
