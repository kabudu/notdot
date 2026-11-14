package dev.notdot.mobile

import java.io.File
import java.io.FileOutputStream
import java.io.IOException

/*
 * Experimental personalised wake-phrase lab: framework-free parts (tested on
 * the JVM). A local diagnostic only: nothing here starts or changes wake
 * listening, and matching is wake-phrase comparison, never identification.
 */

/**
 * Finds one spoken phrase in 20 ms packets of 16 kHz 16-bit audio: waits
 * for speech, keeps 300 ms before it, and ends after 700 ms of quiet, at
 * [maxMs] of speech, or with no speech after 5 s. Bounded memory.
 */
class PhraseEndpointer(private val maxMs: Int) {
    enum class State { WAITING, SPEAKING, DONE, NO_SPEECH }

    private val preRoll = ArrayDeque<ByteArray>()
    private val captured = mutableListOf<ByteArray>()
    private var floor = Float.MAX_VALUE
    private var waitedMs = 0
    private var spokenMs = 0
    private var quietMs = 0
    var state = State.WAITING
        private set

    /** Feed one packet with its RMS level (0..1). Returns the new state. */
    fun push(packet: ByteArray, rms: Float): State {
        if (state == State.DONE || state == State.NO_SPEECH) return state
        val ms = packet.size / 32
        when (state) {
            State.WAITING -> {
                waitedMs += ms
                if (waitedMs <= 300) floor = minOf(floor, rms)
                preRoll.addLast(packet)
                while (preRoll.sumOf { it.size } > PRE_ROLL_BYTES) preRoll.removeFirst()
                if (waitedMs > 300 && rms > threshold()) {
                    state = State.SPEAKING
                    captured += preRoll
                    preRoll.clear()
                    spokenMs = ms
                } else if (waitedMs >= NO_SPEECH_MS) {
                    state = State.NO_SPEECH
                }
            }
            State.SPEAKING -> {
                captured += packet
                spokenMs += ms
                quietMs = if (rms > threshold()) 0 else quietMs + ms
                if ((quietMs >= END_QUIET_MS && spokenMs - quietMs >= 200) || spokenMs >= maxMs) state = State.DONE
            }
            else -> Unit
        }
        return state
    }

    private fun threshold() = maxOf(0.01f, floor * 4f)

    /** The captured phrase (pre-roll included) once [State.DONE]; else empty. */
    fun audio(): ByteArray {
        if (state != State.DONE) return ByteArray(0)
        val out = ByteArray(captured.sumOf { it.size })
        var at = 0
        for (p in captured) {
            p.copyInto(out, at)
            at += p.size
        }
        return out
    }

    companion object {
        const val PRE_ROLL_BYTES = 16_000 * 2 * 3 / 10
        const val END_QUIET_MS = 700
        const val NO_SPEECH_MS = 5_000
        /** Enrolment takes and held-out tests (the core's own limits). */
        const val TAKE_MAX_MS = 3_500
        const val TEST_MAX_MS = 5_500

        fun rms(packet: ByteArray, length: Int = packet.size): Float {
            var sum = 0.0
            var n = 0
            var i = 0
            while (i + 1 < length) {
                val s = ((packet[i + 1].toInt() shl 8) or (packet[i].toInt() and 0xff)).toShort() / 32768.0
                sum += s * s
                n++
                i += 2
            }
            return if (n == 0) 0f else kotlin.math.sqrt(sum / n).toFloat()
        }
    }
}

/**
 * Enrolment in memory: five takes of the phrase, then up to three similar
 * phrases (one take each, optional). Nothing is stored until the caller
 * saves; cancelling only clears memory, so a saved model is untouched.
 */
class PersonalEnrolment(val phrase: String, similar: List<String>) {
    sealed interface Target {
        data class Phrase(val index: Int) : Target
        data class Similar(val label: String) : Target
        data object Review : Target
    }

    private val phraseTakes = arrayOfNulls<ByteArray>(PHRASE_TAKES)
    private val similarTakes = LinkedHashMap<String, ByteArray?>()
    private var skipSimilar = false

    init {
        require(phrase.isNotBlank()) { "Choose a wake phrase first" }
        similar.map { it.trim() }.filter { it.isNotEmpty() && !it.equals(phrase.trim(), ignoreCase = true) }
            .distinctBy { it.lowercase() }.take(MAX_SIMILAR).forEach { similarTakes[it] = null }
    }

    val similarLabels: List<String> get() = similarTakes.keys.toList()
    val phraseCount: Int get() = phraseTakes.count { it != null }
    fun hasSimilar(label: String) = similarTakes[label] != null

    /** What to record next. */
    fun next(): Target {
        phraseTakes.indexOfFirst { it == null }.takeIf { it >= 0 }?.let { return Target.Phrase(it) }
        if (!skipSimilar) similarTakes.entries.firstOrNull { it.value == null }?.let { return Target.Similar(it.key) }
        return Target.Review
    }

    /** Store an accepted take's features (in memory) for [target]. */
    fun accept(target: Target, features: ByteArray) {
        require(features.isNotEmpty())
        when (target) {
            is Target.Phrase -> phraseTakes[target.index] = features
            is Target.Similar -> {
                require(target.label in similarTakes)
                similarTakes[target.label] = features
            }
            Target.Review -> error("nothing to record")
        }
    }

    /** Forget one take so it is recorded again. */
    fun redo(target: Target) {
        when (target) {
            is Target.Phrase -> phraseTakes[target.index] = null
            is Target.Similar -> if (target.label in similarTakes) {
                similarTakes[target.label] = null
                skipSimilar = false
            }
            Target.Review -> Unit
        }
    }

    fun skipRemainingSimilar() {
        skipSimilar = true
    }

    val canSave: Boolean get() = phraseTakes.all { it != null }

    fun phraseFeatures(): List<ByteArray> = phraseTakes.filterNotNull()
    fun similarFeatures(): List<Pair<String, ByteArray>> =
        similarTakes.entries.mapNotNull { (label, f) -> f?.let { label to it } }

    /** Drop every take from memory. */
    fun clear() {
        phraseTakes.fill(null)
        similarTakes.keys.forEach { similarTakes[it] = null }
    }

    companion object {
        const val PHRASE_TAKES = 5
        const val MAX_SIMILAR = 3
    }
}

/** Seals bytes for one file name (the name is bound into the tag). */
interface Sealer {
    fun seal(name: String, data: ByteArray): ByteArray
    /** Throws if the sealed bytes were changed or the key is gone (never creates a key). */
    fun open(name: String, sealed: ByteArray): ByteArray
    /** Throws if the key could not be removed. */
    fun destroyKey()
}

/** Runs work off the main thread one task at a time, and delivers on the main thread. */
interface SerialWorker {
    fun submit(work: () -> Unit)
    fun main(action: () -> Unit)
}

/**
 * Orders the lab's background work so nothing stale ever commits. Every
 * enrolment, cancel and delete moves a token; work captures the token it
 * started under and delivers or writes only if it is still current. A
 * save checks the token and writes under the same lock that [invalidate]
 * takes, so a cancel or delete either stops the write or comes after it,
 * never in between. All store access runs on the one serial worker.
 */
class PersonalLabCoordinator(private val worker: SerialWorker, private val store: PersonalWakeStore) {
    private val lock = Any()
    private var token = 0L

    /** True while a save or delete is in progress (main thread). */
    var busy = false
        private set

    fun current(): Long = synchronized(lock) { token }

    /** Start a new enrolment (or end one): older work is now stale. */
    fun invalidate(): Long = synchronized(lock) { ++token }

    /** Compute off the main thread; deliver only if [id] is still current. */
    fun <T> compute(id: Long, work: () -> T, done: (T) -> Unit) {
        worker.submit {
            val result = work()
            worker.main { if (current() == id) done(result) }
        }
    }

    /** Build and save for enrolment [id]; at most one at a time. False if refused. */
    fun save(id: Long, build: () -> ByteArray, done: (Result<Unit>) -> Unit): Boolean {
        if (busy || current() != id) return false
        busy = true
        worker.submit {
            val result = runCatching {
                val model = build()
                synchronized(lock) {
                    if (token != id) throw java.util.concurrent.CancellationException("cancelled")
                    store.save(model)
                }
            }
            worker.main {
                busy = false
                if (current() == id) done(result)
            }
        }
        return true
    }

    /** Delete everything saved; queued saves can no longer write. */
    fun delete(done: (Result<Unit>) -> Unit) {
        invalidate()
        busy = true
        worker.submit {
            val result = runCatching { synchronized(lock) { store.delete() } }
            worker.main {
                busy = false
                done(result)
            }
        }
    }

    /** Use the store on the worker (serialised with saves and deletes); deliver only if [id] is still current. */
    fun <T> withStore(id: Long, work: (PersonalWakeStore) -> T, done: (T) -> Unit) = compute(id, { work(store) }, done)

    /** Read the saved model on the worker; deliver only if [id] is still current. */
    fun load(id: Long, done: (Result<ByteArray?>) -> Unit) = withStore(id, { runCatching { it.load() } }, done)
}

/**
 * The saved personal model, sealed, in its own no-backup folder
 * (`personal-wake`, beside and never inside the generated `wake` folder,
 * whose other children are deleted on every app update). Writes are
 * atomic: the new model replaces the old only once fully written.
 */
class PersonalWakeStore(private val dir: File, private val sealer: Sealer) {
    private val file get() = File(dir, NAME)

    fun exists() = file.exists()

    /** Null if nothing is saved; throws if the saved file is unreadable. */
    fun load(): ByteArray? {
        val f = file
        if (!f.exists()) return null
        // Bounded before reading: a model is well under this size.
        if (f.length() > MAX_BYTES) throw java.security.GeneralSecurityException("personal wake file is too large")
        return sealer.open(NAME, f.readBytes())
    }

    fun save(model: ByteArray) {
        require(model.isNotEmpty() && model.size <= MAX_BYTES - 64) { "personal wake model size is out of range" }
        val sealed = sealer.seal(NAME, model)
        dir.mkdirs()
        val partial = File(dir, "$NAME.partial")
        try {
            FileOutputStream(partial).use { out ->
                out.write(sealed)
                out.fd.sync()
            }
            if (!partial.renameTo(file)) throw IOException("could not save the personal wake model")
        } finally {
            partial.delete()
        }
    }

    /**
     * Remove the model, any partial write, and the sealing key. Throws if
     * anything could not be removed, so a deletion is never reported that
     * did not happen.
     */
    fun delete() {
        val left = dir.listFiles().orEmpty().count { !it.delete() && it.exists() }
        if (dir.exists() && !dir.delete() && dir.exists()) {
            throw IOException("could not delete ${left.coerceAtLeast(1)} personal wake item(s)")
        }
        sealer.destroyKey()
    }

    companion object {
        const val FOLDER = "personal-wake"
        const val NAME = "model.sealed"
        /** 14 templates x 300 frames x 26 values x 4 bytes is about 440 KB. */
        const val MAX_BYTES = 1L shl 20
    }
}
