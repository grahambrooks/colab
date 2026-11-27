fun run(x: List<Int>, tracks: List<Int>) {
    val empty = !x.isEmpty()
    val total = tracks.fold(0) { acc, t -> acc + t }
    val none = listOf<String>()
    println(empty, total, none)
}
