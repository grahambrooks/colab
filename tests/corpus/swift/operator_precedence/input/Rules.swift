func run(a: Int, b: Box, p: Int, q: Int, r: Bool) throws {
    let y = a + b.f()
    if p == q && r {
        print(y)
    }
    let t = try b.f() + 1
    let n = -b.count
    print(t, n)
}
