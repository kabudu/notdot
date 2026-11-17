package dev.notdot.mobile

import android.media.AudioAttributes
import android.media.MediaDataSource
import android.media.MediaPlayer

/**
 * Plays reply clips one after another from memory (nothing is written to
 * storage). In silent test mode it never plays; it only counts clips.
 */
class ReplyPlayer : ReplySink {
    private val queue = ArrayDeque<ByteArray>()
    private var player: MediaPlayer? = null
    @Volatile var clipsReceived = 0
        private set

    @Synchronized
    override fun play(clip: UInt, mime: String, audio: ByteArray) {
        clipsReceived++
        if (NotdotState.silent) return
        queue.addLast(audio)
        if (player == null) next()
    }

    @Synchronized
    override fun stopAll() {
        queue.clear()
        player?.run { runCatching { stop() }; release() }
        player = null
    }

    @Synchronized
    private fun next() {
        player?.release()
        player = null
        val audio = queue.removeFirstOrNull() ?: return
        val p = MediaPlayer()
        player = p
        try {
            p.setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_ASSISTANT)
                    .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                    .build(),
            )
            p.setDataSource(BytesSource(audio))
            p.setOnCompletionListener { synchronized(this) { if (player === p) next() } }
            p.setOnErrorListener { _, _, _ -> synchronized(this) { if (player === p) next() }; true }
            p.prepare()
            p.start()
        } catch (_: Exception) {
            // A clip that can't play is skipped, not fatal to the reply.
            p.release()
            player = null
            next()
        }
    }

    private class BytesSource(private val bytes: ByteArray) : MediaDataSource() {
        override fun readAt(position: Long, buffer: ByteArray, offset: Int, size: Int): Int {
            if (position >= bytes.size) return -1
            val n = minOf(size.toLong(), bytes.size - position).toInt()
            System.arraycopy(bytes, position.toInt(), buffer, offset, n)
            return n
        }
        override fun getSize(): Long = bytes.size.toLong()
        override fun close() {}
    }
}
