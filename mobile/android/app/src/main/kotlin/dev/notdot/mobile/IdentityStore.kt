package dev.notdot.mobile

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import uniffi.notdot_mobile_core.PhoneLink
import uniffi.notdot_mobile_core.newIdentity
import java.io.File
import java.security.GeneralSecurityException
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * The phone's link identity and paired-desktop record, sealed with an
 * AES-GCM key that never leaves the Android Keystore.
 *
 * Files live in no-backup storage (never in cloud or device-transfer
 * backups). The key needs no user authentication, so a hands-free session
 * can reconnect while the phone is locked and nothing ever prompts. Each
 * file's name is bound into its authentication tag, so files cannot be
 * swapped. The identity is created only when the user starts pairing.
 */
class IdentityStore(private val dir: File, private val alias: String = "notdot-link-wrap") {
    companion object {
        fun standard(context: Context) = IdentityStore(File(context.noBackupFilesDir, "link"))
        private const val IDENTITY = "identity.sealed"
        private const val PAIRED = "paired.sealed"
    }

    fun identity(): ByteArray? = open(IDENTITY)

    /**
     * The stored identity, or a new one sealed and saved before it is
     * returned. Called only when the user starts pairing. If the stored
     * files cannot be read (tampered, or the wrapping key was lost), they
     * are removed and pairing starts afresh rather than failing forever.
     */
    fun identityCreatingIfNeeded(): ByteArray {
        val existing = try {
            identity()
        } catch (e: GeneralSecurityException) {
            clear()
            null
        } catch (e: IllegalArgumentException) {
            clear()
            null
        } catch (e: IllegalStateException) {
            clear()
            null
        }
        return existing ?: newIdentity().also { seal(IDENTITY, it) }
    }

    fun pairedRecord(): String? = open(PAIRED)?.toString(Charsets.UTF_8)

    fun savePairedRecord(record: String?) {
        if (record == null) File(dir, PAIRED).delete() else seal(PAIRED, record.toByteArray(Charsets.UTF_8))
    }

    /** The restored link, or an in-memory one if this phone has no identity yet. */
    fun restoreLink(): PhoneLink = identity()?.let { PhoneLink.restore(it, pairedRecord()) } ?: PhoneLink()

    /** Remove everything, including the wrapping key. */
    fun clear() {
        File(dir, PAIRED).delete()
        File(dir, IDENTITY).delete()
        keystore().deleteEntry(alias)
    }

    private fun keystore() = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    private fun key(): SecretKey = keystore().getKey(alias, null) as SecretKey? ?: KeyGenerator
        .getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        .apply {
            init(
                KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                    .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                    .setKeySize(256)
                    .build(),
            )
        }
        .generateKey()

    private fun seal(name: String, data: ByteArray) {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE, key()) }
        cipher.updateAAD(name.toByteArray(Charsets.UTF_8))
        val sealed = cipher.doFinal(data)
        dir.mkdirs()
        val partial = File(dir, "$name.partial")
        partial.outputStream().use { out ->
            out.write(cipher.iv.size)
            out.write(cipher.iv)
            out.write(sealed)
        }
        check(partial.renameTo(File(dir, name))) { "could not save $name" }
    }

    /** Null if absent; throws if present but not readable (tampered, or key lost). */
    private fun open(name: String): ByteArray? {
        val file = File(dir, name)
        if (!file.exists()) return null
        val bytes = file.readBytes()
        val ivSize = bytes.firstOrNull()?.toInt() ?: error("$name is empty")
        require(ivSize in 12..16 && bytes.size > 1 + ivSize) { "$name is malformed" }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply {
            init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, bytes, 1, ivSize))
        }
        cipher.updateAAD(name.toByteArray(Charsets.UTF_8))
        return cipher.doFinal(bytes, 1 + ivSize, bytes.size - 1 - ivSize)
    }
}
