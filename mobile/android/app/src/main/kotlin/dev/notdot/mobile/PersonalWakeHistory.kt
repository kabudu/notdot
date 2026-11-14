package dev.notdot.mobile

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream

/** Result metadata only. Never contains audio or acoustic features. */
internal data class WakeTrial(val time: Long, val expected: String, val outcome: String)

/** A bounded, versioned journal, sealed by the caller in a separate Keystore store. */
internal object WakeTrialHistory {
    const val LIMIT = 100
    fun append(previous: List<WakeTrial>, trial: WakeTrial) = (previous + trial).takeLast(LIMIT)
    fun encode(trials: List<WakeTrial>): ByteArray {
        require(trials.size <= LIMIT)
        return ByteArrayOutputStream().also { buffer ->
            DataOutputStream(buffer).use { out ->
                out.writeInt(1)
                out.writeInt(trials.size)
                trials.forEach {
                    require(it.expected.length <= 120 && it.outcome.length <= 1000)
                    out.writeLong(it.time)
                    out.writeUTF(it.expected)
                    out.writeUTF(it.outcome)
                }
            }
        }.toByteArray()
    }
    fun decode(bytes: ByteArray): List<WakeTrial> {
        require(bytes.size <= 256_000)
        return DataInputStream(ByteArrayInputStream(bytes)).use { input ->
            require(input.readInt() == 1) { "Unsupported history version" }
            val count = input.readInt()
            require(count in 0..LIMIT)
            val trials = List(count) {
                WakeTrial(input.readLong(), input.readUTF(), input.readUTF()).also {
                    require(it.expected.length <= 120 && it.outcome.length <= 1000)
                }
            }
            require(input.available() == 0) { "Unexpected history data" }
            trials
        }
    }
}
