// A music playlist: the property suite's Kotlin sample.
package example.music

import java.time.Duration
import kotlin.math.max
import kotlin.random.Random

enum class Genre { ROCK, JAZZ, CLASSICAL, POP }

data class Track(
    val title: String,
    val artist: String,
    val length: Duration,
    val genre: Genre = Genre.POP,
    val rating: Int = 0,
)

sealed class Command {
    data class Add(val track: Track) : Command()
    data class Remove(val index: Int) : Command()
    object Shuffle : Command()
    object Clear : Command()
}

class PlaylistException(message: String) : RuntimeException(message)

interface Listener {
    fun changed(playlist: Playlist)
}

open class Playlist(val name: String) : Iterable<Track> {
    private val tracks = mutableListOf<Track>()
    private val listeners = mutableListOf<Listener>()
    var played: Int = 0
        private set

    val totalLength: Duration
        get() = tracks.fold(Duration.ZERO) { sum, track -> sum.plus(track.length) }

    val size: Int get() = tracks.size

    override fun iterator(): Iterator<Track> = tracks.iterator()

    fun listen(listener: Listener) {
        listeners += listener
    }

    fun apply(command: Command) {
        when (command) {
            is Command.Add -> tracks.add(command.track)
            is Command.Remove -> {
                if (command.index !in tracks.indices) {
                    throw PlaylistException("no track ${command.index} in $name")
                }
                tracks.removeAt(command.index)
            }
            Command.Shuffle -> tracks.shuffle(Random(42))
            Command.Clear -> tracks.clear()
        }
        listeners.forEach { it.changed(this) }
    }

    fun byArtist(): Map<String, List<Track>> = tracks.groupBy { it.artist }

    fun best(count: Int = 3): List<Track> =
        tracks.sortedWith(compareByDescending<Track> { it.rating }.thenBy { it.title }).take(count)

    open fun next(): Track? {
        if (tracks.isEmpty()) return null
        val track = tracks[played % tracks.size]
        played = max(played + 1, 0)
        return track
    }

    fun describe(genre: Genre): String = when (genre) {
        Genre.ROCK -> "loud"
        Genre.JAZZ, Genre.CLASSICAL -> "calm"
        else -> "catchy"
    }

    companion object {
        const val MAX_TRACKS = 500

        fun of(name: String, vararg tracks: Track): Playlist =
            Playlist(name).apply { tracks.forEach { apply(Command.Add(it)) } }
    }
}

class RepeatingPlaylist(name: String, private val times: Int) : Playlist(name) {
    private var rounds = 0

    override fun next(): Track? {
        val track = super.next() ?: return null
        if (played % size == 0) {
            rounds++
        }
        return if (rounds < times) track else null
    }
}

fun Duration.pretty(): String {
    val minutes = toMinutes()
    val seconds = seconds % 60
    return "%d:%02d".format(minutes, seconds)
}

suspend fun loadTracks(fetch: suspend (String) -> List<Track>, names: List<String>): List<Track> {
    val all = mutableListOf<Track>()
    for (name in names) {
        try {
            all += fetch(name)
        } catch (e: PlaylistException) {
            println("skipped $name: ${e.message}")
        }
    }
    return all
}

fun main() {
    val playlist = Playlist.of(
        "Sunday",
        Track("Blue in Green", "Miles Davis", Duration.ofSeconds(337), Genre.JAZZ, 5),
        Track("Clair de Lune", "Debussy", Duration.ofSeconds(300), Genre.CLASSICAL, 4),
    )
    playlist.listen(object : Listener {
        override fun changed(playlist: Playlist) = println("${playlist.name}: ${playlist.size} tracks")
    })
    playlist.apply(Command.Shuffle)
    println(playlist.totalLength.pretty() + " " + playlist.best(1).map { it.title })
}
