pub fn run() {
    let _ = new_fn(1, 2);
    let _ = new_fn(3, 4);
    other_fn(5);
    // x.old_fn(6) is a method call and is left untouched.
    let _ = x.old_fn(6);
}
