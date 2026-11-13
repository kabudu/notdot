package dev.notdot.mobile

import android.app.Activity
import android.hardware.biometrics.BiometricManager.Authenticators.BIOMETRIC_STRONG
import android.hardware.biometrics.BiometricPrompt
import android.os.Build
import android.os.CancellationSignal
import android.os.Looper
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyInfo
import android.security.keystore.KeyProperties
import android.security.keystore.StrongBoxUnavailableException
import uniffi.notdot_mobile_core.KeyProtection
import uniffi.notdot_mobile_core.MobileException
import uniffi.notdot_mobile_core.StepUpKey
import uniffi.notdot_mobile_core.StepUpSigner
import java.math.BigInteger
import java.security.KeyFactory
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.PrivateKey
import java.security.Signature
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * The owner step-up key: P-256 in the Android Keystore, in StrongBox when
 * the phone has it, otherwise in the trusted execution environment (TEE).
 *
 * Every signature needs a strong biometric (fingerprint or face) and a new
 * enrolment invalidates the key. Creating or finding the key never shows a
 * prompt; only [signer]'s `sign`, for an action the owner just started.
 *
 * Where the key lives is read back from the Keystore ([KeyInfo]), never
 * assumed, and a key the Keystore reports as software is deleted and
 * refused: there is no silent downgrade. Before Android 12 the Keystore
 * cannot say StrongBox, so such keys are reported as the TEE (an
 * under-report, never an over-report).
 */
class StepUpKeyStore(private val alias: String = "notdot-step-up") {
    class NoSecureHardware(message: String) : Exception(message)

    /** The existing key or a new one, and where it lives. Never prompts. */
    fun key(): StepUpKey {
        val entry = privateEntry() ?: create()
        val protection = protectionOf(entry.privateKey)
        if (protection == KeyProtection.SOFTWARE) {
            delete()
            throw NoSecureHardware("This phone keeps keys in software only, so sensitive actions are unavailable on it.")
        }
        return StepUpKey(sec1(entry.certificate.publicKey as ECPublicKey), protection)
    }

    fun delete() = keystore().deleteEntry(alias)

    /** Signs challenges with this key; each signature asks for a biometric. */
    fun signer(activity: Activity, title: String): StepUpSigner = object : StepUpSigner {
        override fun sign(message: ByteArray): ByteArray {
            check(Looper.myLooper() != Looper.getMainLooper()) { "step-up signing blocks; call it off the main thread" }
            val entry = privateEntry() ?: throw MobileException.Invalid("This phone has no step-up key; link it again.")
            val signature = try {
                Signature.getInstance("SHA256withECDSA").apply { initSign(entry.privateKey) }
            } catch (e: Exception) {
                // e.g. KeyPermanentlyInvalidatedException after a new enrolment.
                throw MobileException.Invalid("This phone's step-up key is no longer valid; link it again.")
            }
            val done = CountDownLatch(1)
            var signed: ByteArray? = null
            var failure = "Not confirmed."
            val cancel = CancellationSignal()
            activity.mainExecutor.execute {
                BiometricPrompt.Builder(activity)
                    .setTitle(title)
                    .apply { if (Build.VERSION.SDK_INT >= 30) setAllowedAuthenticators(BIOMETRIC_STRONG) }
                    .setNegativeButton("Cancel", activity.mainExecutor) { _, _ -> failure = "Cancelled."; done.countDown() }
                    .build()
                    .authenticate(BiometricPrompt.CryptoObject(signature), cancel, activity.mainExecutor,
                        object : BiometricPrompt.AuthenticationCallback() {
                            override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                                signed = runCatching {
                                    result.cryptoObject.signature?.run { update(message); sign() }
                                }.getOrNull()
                                done.countDown()
                            }

                            override fun onAuthenticationError(code: Int, text: CharSequence) {
                                failure = text.toString()
                                done.countDown()
                            }
                        })
            }
            // Within the desktop's challenge lifetime (60 s).
            if (!done.await(55, TimeUnit.SECONDS)) {
                cancel.cancel()
                throw MobileException.Invalid("Not confirmed in time.")
            }
            return signed ?: throw MobileException.Invalid(failure)
        }
    }

    private fun keystore() = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    private fun privateEntry() = keystore().getEntry(alias, null) as? KeyStore.PrivateKeyEntry

    private fun create(): KeyStore.PrivateKeyEntry {
        try {
            generate(strongBox = true)
        } catch (_: StrongBoxUnavailableException) {
            generate(strongBox = false)
        }
        return privateEntry() ?: throw NoSecureHardware("The step-up key could not be created.")
    }

    private fun generate(strongBox: Boolean) {
        val spec = KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_SIGN)
            .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
            .setDigests(KeyProperties.DIGEST_SHA256)
            .setUserAuthenticationRequired(true)
            .apply {
                if (Build.VERSION.SDK_INT >= 30) {
                    setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG)
                }
            }
            .setInvalidatedByBiometricEnrollment(true)
            .setIsStrongBoxBacked(strongBox)
            .build()
        try {
            KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore").run {
                initialize(spec)
                generateKeyPair()
            }
        } catch (e: StrongBoxUnavailableException) {
            throw e
        } catch (e: Exception) {
            // Typically no screen lock or no enrolled strong biometric.
            throw NoSecureHardware("Set a screen lock and enrol a fingerprint or face to confirm sensitive actions.")
        }
    }

    companion object {
        /** Where the Keystore says this key lives. */
        fun protectionOf(key: PrivateKey): KeyProtection {
            val info = KeyFactory.getInstance(key.algorithm, "AndroidKeyStore").getKeySpec(key, KeyInfo::class.java)
            if (Build.VERSION.SDK_INT >= 31) {
                return when (info.securityLevel) {
                    KeyProperties.SECURITY_LEVEL_STRONGBOX -> KeyProtection.STRONG_BOX
                    // Secure hardware of unknown kind: at least a TEE.
                    KeyProperties.SECURITY_LEVEL_TRUSTED_ENVIRONMENT,
                    KeyProperties.SECURITY_LEVEL_UNKNOWN_SECURE -> KeyProtection.TRUSTED_ENVIRONMENT
                    else -> KeyProtection.SOFTWARE
                }
            }
            @Suppress("DEPRECATION")
            return if (info.isInsideSecureHardware) KeyProtection.TRUSTED_ENVIRONMENT else KeyProtection.SOFTWARE
        }

        /** SEC1 uncompressed point (65 bytes), as the core expects. */
        fun sec1(key: ECPublicKey): ByteArray = byteArrayOf(4) + fixed(key.w.affineX) + fixed(key.w.affineY)

        /** A coordinate as exactly 32 big-endian bytes. */
        internal fun fixed(n: BigInteger): ByteArray {
            val bytes = n.toByteArray().dropWhile { it == 0.toByte() }.toByteArray()
            require(bytes.size <= 32) { "not a P-256 coordinate" }
            return ByteArray(32 - bytes.size) + bytes
        }
    }
}
