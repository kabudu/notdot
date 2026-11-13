package dev.notdot.mobile

import android.app.Activity
import android.content.Context
import android.hardware.biometrics.BiometricManager.Authenticators.BIOMETRIC_STRONG
import android.hardware.biometrics.BiometricPrompt
import android.os.Build
import android.os.CancellationSignal
import android.os.Looper
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import android.security.keystore.StrongBoxUnavailableException
import uniffi.notdot_mobile_core.AttestRequest
import uniffi.notdot_mobile_core.MobileException
import uniffi.notdot_mobile_core.SignRequest
import uniffi.notdot_mobile_core.StepUpAttester
import uniffi.notdot_mobile_core.StepUpEvidence
import uniffi.notdot_mobile_core.StepUpSignature
import uniffi.notdot_mobile_core.StepUpSigner
import java.math.BigInteger
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.Signature
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * The owner step-up key: P-256 in the Android Keystore, in StrongBox when
 * the phone has it, otherwise wherever the Keystore puts it.
 *
 * Every signature needs a strong biometric (fingerprint or face) and a new
 * enrolment invalidates the key. Creating the key never shows a prompt; only
 * [signer]'s `sign`, for an action the owner just confirmed.
 *
 * The key is created with the computer's attestation challenge, and its
 * certificate chain is sent as evidence. The computer checks it against
 * Google's roots and decides the protection level; the phone's own view of
 * its hardware is never trusted. A key the computer refuses is deleted.
 */
object StepUpKeys {
    private const val PREFS = "notdot-step-up"
    private const val ACTIVE = "alias"
    private const val PENDING = "pending"

    private fun keystore() = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    fun activeAlias(context: Context): String? =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(ACTIVE, null)

    /** Make `alias` current and delete the previous key. */
    fun activate(context: Context, alias: String) {
        val previous = activeAlias(context)
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString(ACTIVE, alias).commit()
        if (previous != null && previous != alias) delete(previous)
    }

    /** The computer may have accepted `alias` but the reply was lost:
     *  keep it until [reconcile] can tell. */
    fun park(context: Context, alias: String) {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        prefs.getString(PENDING, null)?.takeIf { it != alias }?.let(::delete)
        prefs.edit().putString(PENDING, alias).commit()
    }

    /** Settle a pending key from the key the computer says it holds. */
    fun reconcile(context: Context, computerHolds: ByteArray?) {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val pending = prefs.getString(PENDING, null) ?: return
        val public = runCatching {
            sec1(keystore().getCertificate(pending).publicKey as ECPublicKey)
        }.getOrNull()
        prefs.edit().remove(PENDING).commit()
        if (computerHolds != null && public != null &&
            uniffi.notdot_mobile_core.stepUpKeySha256(public).contentEquals(computerHolds)
        ) {
            activate(context, pending)
        } else {
            delete(pending)
        }
    }

    /** Delete every step-up key (the computer removed this phone). */
    fun clear(context: Context) {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        activeAlias(context)?.let(::delete)
        prefs.getString(PENDING, null)?.let(::delete)
        prefs.edit().clear().commit()
    }

    fun delete(alias: String) = keystore().deleteEntry(alias)

    /** Create a key bound to `challenge`, StrongBox first. Never prompts. */
    fun create(alias: String, challenge: ByteArray) {
        try {
            generate(alias, challenge, strongBox = true)
        } catch (_: StrongBoxUnavailableException) {
            generate(alias, challenge, strongBox = false)
        }
    }

    /** The key's attestation chain, leaf first. */
    fun chain(alias: String): List<ByteArray> =
        keystore().getCertificateChain(alias)?.map { it.encoded }
            ?: throw MobileException.Unavailable("This phone couldn't produce evidence for its key.")

    private fun generate(alias: String, challenge: ByteArray, strongBox: Boolean) {
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
            .setAttestationChallenge(challenge)
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
            android.util.Log.w("notdot", "step-up key generation failed (strongBox=$strongBox)", e)
            throw generationFailure(e)
        }
    }

    /** Plain words for why a key couldn't be made. Attestation keys come
     *  from Google's remote provisioning, so a phone that has never been
     *  online may have none yet: that is temporary, not a hardware gap. */
    internal fun generationFailure(e: Exception): MobileException {
        val keystore = generateSequence(e as Throwable) { it.cause }
            .firstOrNull { it is android.security.KeyStoreException }
        if (Build.VERSION.SDK_INT >= 33 && keystore is android.security.KeyStoreException && keystore.isTransientFailure) {
            return MobileException.Offline(
                "This phone needs to be online once to get its security certificates from Google. Connect to the internet and try again.")
        }
        if (e is java.security.InvalidAlgorithmParameterException || e is IllegalStateException) {
            return MobileException.Unavailable(
                "Set a screen lock and add a fingerprint or face to confirm sensitive actions.")
        }
        return MobileException.Unavailable("This phone couldn't create a protected key, so sensitive actions stay off.")
    }

    /** SEC1 uncompressed point (65 bytes), as the core expects. */
    fun sec1(key: ECPublicKey): ByteArray = byteArrayOf(4) + fixed(key.w.affineX) + fixed(key.w.affineY)

    /** A coordinate as exactly 32 big-endian bytes. */
    internal fun fixed(n: BigInteger): ByteArray {
        val bytes = n.toByteArray().dropWhile { it == 0.toByte() }.toByteArray()
        require(bytes.size <= 32) { "not a P-256 coordinate" }
        return ByteArray(32 - bytes.size) + bytes
    }

    /** Signs one action after a strong biometric. */
    fun signer(activity: Activity, title: String, subtitle: String): StepUpSigner = object : StepUpSigner {
        override fun sign(request: SignRequest): StepUpSignature {
            check(Looper.myLooper() != Looper.getMainLooper()) { "step-up signing blocks; call it off the main thread" }
            val alias = activeAlias(activity)
                ?: throw MobileException.Unavailable("Set up sensitive actions on this phone first.")
            val entry = keystore().getEntry(alias, null) as? KeyStore.PrivateKeyEntry
                ?: throw MobileException.Unavailable("Set up sensitive actions on this phone first.")
            val signature = try {
                Signature.getInstance("SHA256withECDSA").apply { initSign(entry.privateKey) }
            } catch (e: KeyPermanentlyInvalidatedException) {
                throw MobileException.Unavailable(
                    "Your fingerprints or face changed, so this phone's key stopped working. Set up sensitive actions again.")
            } catch (e: Exception) {
                throw MobileException.Unavailable("This phone's key can't be used. Set up sensitive actions again.")
            }
            val done = CountDownLatch(1)
            var signed: ByteArray? = null
            var outcome: MobileException? = null
            val cancel = CancellationSignal()
            activity.mainExecutor.execute {
                BiometricPrompt.Builder(activity)
                    .setTitle(title)
                    .setSubtitle(subtitle)
                    .apply { if (Build.VERSION.SDK_INT >= 30) setAllowedAuthenticators(BIOMETRIC_STRONG) }
                    .setNegativeButton("Cancel", activity.mainExecutor) { _, _ ->
                        outcome = MobileException.Cancelled("Not confirmed; nothing changed.")
                        done.countDown()
                    }
                    .build()
                    .authenticate(BiometricPrompt.CryptoObject(signature), cancel, activity.mainExecutor,
                        object : BiometricPrompt.AuthenticationCallback() {
                            override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                                signed = runCatching {
                                    result.cryptoObject.signature?.run { update(request.message); sign() }
                                }.getOrNull()
                                done.countDown()
                            }

                            override fun onAuthenticationError(code: Int, text: CharSequence) {
                                outcome = when (code) {
                                    BiometricPrompt.BIOMETRIC_ERROR_USER_CANCELED,
                                    BiometricPrompt.BIOMETRIC_ERROR_CANCELED ->
                                        MobileException.Cancelled("Not confirmed; nothing changed.")
                                    BiometricPrompt.BIOMETRIC_ERROR_NO_BIOMETRICS,
                                    BiometricPrompt.BIOMETRIC_ERROR_HW_NOT_PRESENT,
                                    BiometricPrompt.BIOMETRIC_ERROR_HW_UNAVAILABLE ->
                                        MobileException.Unavailable(text.toString())
                                    else -> MobileException.Denied(text.toString())
                                }
                                done.countDown()
                            }
                        })
            }
            // Within the computer's challenge lifetime (60 s).
            if (!done.await(55, TimeUnit.SECONDS)) {
                cancel.cancel()
                throw MobileException.Expired("Not confirmed in time; nothing changed.")
            }
            outcome?.let { throw it }
            return StepUpSignature(signed ?: throw MobileException.Denied("Not confirmed."), null)
        }
    }
}

/** Creates a fresh key for the computer's challenge and returns its chain. */
class AndroidAttester(private val context: Context) : StepUpAttester {
    @Volatile var created: String? = null
        private set

    override fun attest(request: AttestRequest): StepUpEvidence {
        val alias = "notdot-step-up-${UUID.randomUUID()}"
        StepUpKeys.create(alias, request.androidChallenge)
        created = alias
        return StepUpEvidence.AndroidKeystore(StepUpKeys.chain(alias))
    }

    /** Keep the new key if the computer accepted it (`failure` null); park
     *  it if the reply was lost (settled at the next status check); else
     *  delete it. */
    fun finish(failure: Exception?) {
        val alias = created ?: return
        when (failure) {
            null -> StepUpKeys.activate(context, alias)
            is MobileException.Offline -> StepUpKeys.park(context, alias)
            else -> StepUpKeys.delete(alias)
        }
    }
}
