fun run(x: List<Int>, tracks: List<Int>) {
    val empty = !isEmpty(x)
    val total = tracks.foldLeft(0) { acc, t -> acc + t }
    val none = emptyList<String>()
    println(empty, total, none)
}
