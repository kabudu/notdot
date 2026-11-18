package dev.notdot.mobile

/**
 * Wake is suppressed while the phone plays a reply, and for [holdMs] after
 * it stops (room echo and the speaker's tail), so the assistant cannot wake
 * itself by saying its own name. A detector that keeps state is restarted
 * when suppression ends ([resumed]), so audio from either side of the reply
 * is never joined. Thread-safe; [clock] is elapsed milliseconds.
 */
class PlaybackGate(private val clock: () -> Long, private val holdMs: Long = HOLD_MS) {
    private var playing = false
    private var quietUntil = Long.MIN_VALUE
    private var wasSuppressed = false

    /** Called by the reply player when playback starts or ends. */
    @Synchronized
    fun playback(active: Boolean) {
        if (playing && !active) quietUntil = clock() + holdMs
        playing = active
    }

    /** True while wake must be ignored. */
    @Synchronized
    fun suppressed(): Boolean {
        val now = playing || clock() < quietUntil
        if (now) wasSuppressed = true
        return now
    }

    /** True once when suppression has ended since the last call. */
    @Synchronized
    fun resumed(): Boolean {
        if (!wasSuppressed || suppressed()) return false
        wasSuppressed = false
        return true
    }

    companion object {
        const val HOLD_MS = 400L
    }
}

/** "hey harriet" as saved by the core → "Hey Harriet" for the status line. */
fun displayPhrase(phrase: String): String =
    phrase.split(' ').filter { it.isNotEmpty() }.joinToString(" ") { w -> w.replaceFirstChar { it.uppercase() } }
