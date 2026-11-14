package dev.notdot.mobile

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream

/**
 * Result metadata only. Never contains audio or acoustic features.
 * [detector] names the detector and version that produced the outcome.
 */
internal data class WakeTrial(
    val time: Long,
    val expected: String,
    val outcome: String,
    val detector: String = LabDetector.MFCC.id,
)

/**
 * A bounded, versioned journal, sealed by the caller in a separate Keystore
 * store. Version 2 adds the detector id; version 1 journals (written before
 * there was more than one detector) still decode, as the MFCC detector's.
 */
internal object WakeTrialHistory {
    const val LIMIT = 100
    private const val VERSION = 2
    private const val MAX_DETECTOR = 40
    /**
     * Largest encoded journal, for encode and decode alike. A full journal
     * of maximal fields (modified UTF-8 takes up to 3 bytes per char) is
     * 8 + 100 x (8 + 362 + 3002 + 122) = 349,408 bytes; the cap leaves room
     * and stays well inside the sealed store's 1 MB file limit.
     */
    const val MAX_BYTES = 512_000

    fun append(previous: List<WakeTrial>, trial: WakeTrial) = (previous + trial).takeLast(LIMIT)

    fun encode(trials: List<WakeTrial>): ByteArray {
        require(trials.size <= LIMIT)
        return ByteArrayOutputStream().also { buffer ->
            DataOutputStream(buffer).use { out ->
                out.writeInt(VERSION)
                out.writeInt(trials.size)
                trials.forEach {
                    require(it.expected.length <= 120 && it.outcome.length <= 1000 && it.detector.length in 1..MAX_DETECTOR)
                    out.writeLong(it.time)
                    out.writeUTF(it.expected)
                    out.writeUTF(it.outcome)
                    out.writeUTF(it.detector)
                }
            }
        }.toByteArray().also { require(it.size <= MAX_BYTES) { "History is too large" } }
    }

    fun decode(bytes: ByteArray): List<WakeTrial> {
        require(bytes.size <= MAX_BYTES)
        return DataInputStream(ByteArrayInputStream(bytes)).use { input ->
            val version = input.readInt()
            require(version == 1 || version == VERSION) { "Unsupported history version" }
            val count = input.readInt()
            require(count in 0..LIMIT)
            val trials = List(count) {
                val time = input.readLong()
                val expected = input.readUTF()
                val outcome = input.readUTF()
                val detector = if (version == 1) LabDetector.MFCC.id else input.readUTF()
                require(expected.length <= 120 && outcome.length <= 1000 && detector.length in 1..MAX_DETECTOR)
                WakeTrial(time, expected, outcome, detector)
            }
            require(input.available() == 0) { "Unexpected history data" }
            trials
        }
    }
}
