package dev.notdot.mobile

import android.Manifest
import android.content.pm.PackageManager
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableDoubleStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import uniffi.notdot_mobile_core.ContrastTake
import uniffi.notdot_mobile_core.MobileException
import uniffi.notdot_mobile_core.OrbMode
import uniffi.notdot_mobile_core.PersonalMatch
import uniffi.notdot_mobile_core.PersonalTake
import uniffi.notdot_mobile_core.TakeIssue
import uniffi.notdot_mobile_core.personalMatch
import uniffi.notdot_mobile_core.personalModel
import uniffi.notdot_mobile_core.personalModelSummary
import uniffi.notdot_mobile_core.personalTake
import java.io.File
import java.security.GeneralSecurityException
import java.security.KeyStore
import java.util.concurrent.Executors
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import kotlin.concurrent.thread

/**
 * EXPERIMENTAL personal wake-phrase lab (builds with
 * -PnotdotPersonalWakeLab=true only). Records the user's phrase, keeps only
 * its sound pattern (sealed, on this phone) and lets them try it. It never
 * starts, changes or feeds wake listening, and it cannot tell who is
 * speaking. Background work goes through [PersonalLabCoordinator], so a
 * cancelled or replaced enrolment never commits.
 */
class PersonalWakeActivity : ComponentActivity() {
    private val main = Handler(Looper.getMainLooper())
    private val executor = Executors.newSingleThreadExecutor { Thread(it, "notdot-personal-lab").apply { isDaemon = true } }
    private val coordinator by lazy {
        PersonalLabCoordinator(
            object : SerialWorker {
                override fun submit(work: () -> Unit) {
                    executor.execute(work)
                }
                override fun main(action: () -> Unit) {
                    main.post { if (!isDestroyed) action() }
                }
            },
            PersonalWakeStore(File(noBackupFilesDir, PersonalWakeStore.FOLDER), KeystoreSealer()),
        )
    }

    // UI state (main thread only).
    private var screen by mutableStateOf<Screen>(Screen.Home)
    private var message by mutableStateOf("")
    private var level by mutableDoubleStateOf(0.0)
    private var phrase by mutableStateOf("Hey Ava")
    private var similar by mutableStateOf("Hey Ada, Hey Eva, Hey Nova")
    private var saved by mutableStateOf<String?>(null)
    private var revision by mutableIntStateOf(0)
    private var showDetails by mutableStateOf(false)
    private var enrolment: PersonalEnrolment? = null
    private var enrolmentId = 0L
    private var lastMatch by mutableStateOf<PersonalMatch?>(null)

    // The one microphone owner. A request generation guards every step: a
    // newer request, stop, hide or cancel makes older callbacks no-ops.
    private var generation = 0L
    private var pendingStart: Job? = null
    /** One recording-and-processing attempt; late results of older ones are dropped. */
    private val attempts = Attempts()
    @Volatile private var recorder: AudioRecord? = null

    private sealed interface Screen {
        data object Home : Screen
        data class Recording(val target: PersonalEnrolment.Target?, val listening: Boolean) : Screen
        data object Review : Screen
        data object Test : Screen
    }

    private val permission = registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        message = if (granted) "Microphone allowed. Tap Record again." else "Microphone access is off; nothing was recorded."
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Debug lab builds only, whatever started this activity.
        if (!(BuildConfig.DEBUG && BuildConfig.PERSONAL_WAKE_LAB)) {
            finish()
            return
        }
        refreshSaved()
        setContent { MaterialTheme(colorScheme = darkColorScheme()) { Lab() } }
    }

    override fun onStop() {
        // Hidden: release the microphone (and any pending start); an
        // unsaved enrolment stays in memory until cancelled.
        val wasRecording = recorder != null || pendingStart != null
        stopRecording()
        if (wasRecording || screen is Screen.Recording) {
            screen = if (enrolment != null) Screen.Review else Screen.Home
            message = "Recording stopped because the app was hidden. Nothing from it was kept."
        }
        super.onStop()
    }

    override fun onDestroy() {
        stopRecording()
        enrolment?.clear()
        executor.shutdown()
        super.onDestroy()
    }

    private fun refreshSaved() {
        val id = coordinator.current()
        val forPhrase = phrase
        coordinator.load(id) { result ->
            if (phrase != forPhrase) return@load
            saved = result.fold(
                onSuccess = { model ->
                    model?.let {
                        try {
                            val s = personalModelSummary(it, forPhrase)
                            "Saved for “$forPhrase”: ${s.phraseTakes} recordings" +
                                if (s.contrastLabels.isEmpty()) "" else ", similar: ${s.contrastLabels.joinToString()}"
                        } catch (e: MobileException) {
                            e.message ?: "Saved recordings cannot be used"
                        }
                    }
                },
                onFailure = { "Saved recordings cannot be read on this phone. Delete them and record again." },
            )
        }
    }

    // ---- Microphone ownership ----

    private fun withMicrophone(start: (Long) -> Unit) {
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            attempts.invalidate()
            permission.launch(Manifest.permission.RECORD_AUDIO)
            return
        }
        if (NotdotState.silent) {
            attempts.invalidate()
            message = "Silent test mode: the microphone stays off."
            return
        }
        releaseMicrophone()
        val request = generation
        pendingStart = lifecycleScope.launch {
            // One microphone owner: pause hands-free and wait until it has
            // released the microphone before recording.
            if (NotdotState.effects.value.microphone) {
                message = "Pausing hands-free listening…"
                VoiceSessionService.stop(this@PersonalWakeActivity)
                val released = withTimeoutOrNull(3_000) { NotdotState.effects.first { !it.microphone } }
                if (released == null) {
                    if (generation == request) recordingFailed(request, "Hands-free listening did not stop; nothing was recorded.")
                    return@launch
                }
            }
            // Still wanted, still visible, and nothing newer asked for the mic.
            if (generation != request || !lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)) return@launch
            pendingStart = null
            start(request)
        }
    }

    private fun record(request: Long, maxMs: Int, onAudio: (ByteArray) -> Unit) {
        if (generation != request) return
        val rate = RATE
        var created: AudioRecord? = null
        val record = try {
            val size = AudioRecord.getMinBufferSize(rate, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
            check(size > 0)
            @Suppress("MissingPermission")
            created = AudioRecord(MediaRecorder.AudioSource.VOICE_RECOGNITION, rate, AudioFormat.CHANNEL_IN_MONO,
                AudioFormat.ENCODING_PCM_16BIT, maxOf(size, 640 * 4))
            check(created.state == AudioRecord.STATE_INITIALIZED)
            created.startRecording()
            check(created.recordingState == AudioRecord.RECORDSTATE_RECORDING)
            created
        } catch (_: RuntimeException) {
            // Release a partly created recorder before reporting.
            created?.let { runCatching { it.stop() }; it.release() }
            recordingFailed(request, "The microphone could not start. Nothing was recorded.")
            return
        }
        recorder = record
        thread(name = "notdot-personal-mic") {
            val endpointer = PhraseEndpointer(maxMs)
            val packet = ByteArray(640)
            var audio: ByteArray? = null
            var noSpeech = false
            try {
                while (recorder === record) {
                    val read = record.read(packet, 0, packet.size, AudioRecord.READ_BLOCKING)
                    if (read != packet.size || recorder !== record) break
                    val rms = PhraseEndpointer.rms(packet)
                    val state = endpointer.push(packet.copyOf(), rms)
                    main.post {
                        if (generation != request) return@post
                        level = (rms * 12f).coerceIn(0f, 1f).toDouble()
                        (screen as? Screen.Recording)?.let {
                            val listening = state == PhraseEndpointer.State.SPEAKING
                            if (it.listening != listening) screen = it.copy(listening = listening)
                        }
                    }
                    if (state == PhraseEndpointer.State.DONE) {
                        audio = endpointer.audio()
                        break
                    }
                    if (state == PhraseEndpointer.State.NO_SPEECH) {
                        noSpeech = true
                        break
                    }
                }
            } catch (_: RuntimeException) {
                // Reported below.
            }
            val captured = audio
            main.post {
                if (generation != request) {
                    captured?.fill(0)
                    return@post
                }
                releaseMicrophone() // the attempt continues into processing
                when {
                    captured != null -> onAudio(captured)
                    noSpeech -> recordingFailed(null, "No speech heard. Try again a little closer.")
                    else -> recordingFailed(null, "Recording stopped. Nothing was kept.")
                }
            }
        }
    }

    private fun recordingFailed(request: Long?, text: String) {
        if (request != null && generation != request) return
        stopRecording()
        message = text
        val s = screen
        screen = if (s is Screen.Recording) s.copy(listening = false) else s
    }

    /** End the current attempt and release the microphone; late results are dropped. */
    private fun stopRecording() {
        attempts.invalidate()
        releaseMicrophone()
    }

    /** Release the microphone and cancel any pending start (the attempt may continue). */
    private fun releaseMicrophone() {
        generation++
        pendingStart?.cancel()
        pendingStart = null
        level = 0.0
        recorder?.let {
            recorder = null
            runCatching { it.stop() }
            it.release()
        }
    }

    // ---- Enrolment ----

    private fun startEnrolment() {
        if (coordinator.busy.value) return
        stopRecording()
        val labels = similar.split(',').map { it.trim() }.filter { it.isNotEmpty() }
        val e = try {
            PersonalEnrolment(phrase.trim(), labels)
        } catch (ex: IllegalArgumentException) {
            message = ex.message ?: "Choose a wake phrase first"
            return
        }
        enrolment?.clear()
        enrolment = e
        enrolmentId = coordinator.invalidate()
        message = ""
        showNext()
    }

    private fun showNext() {
        val e = enrolment ?: return
        revision++
        screen = when (val target = e.next()) {
            PersonalEnrolment.Target.Review -> Screen.Review
            else -> Screen.Recording(target, false)
        }
    }

    private fun recordTarget(target: PersonalEnrolment.Target) {
        if (coordinator.busy.value) return
        stopRecording()
        val id = enrolmentId
        val attempt = attempts.begin()
        withMicrophone { request ->
            record(request, PhraseEndpointer.TAKE_MAX_MS) { audio -> checkTake(id, attempt, target, audio) }
        }
    }

    private fun checkTake(id: Long, attempt: Long, target: PersonalEnrolment.Target, audio: ByteArray) {
        message = "Checking…"
        coordinator.compute(id, {
            try {
                personalTake(audio, RATE.toUInt())
            } catch (_: MobileException) {
                null
            } finally {
                audio.fill(0) // the recording itself is never kept
            }
        }) { take: PersonalTake? ->
            attempts.finish(attempt) {
                val e = enrolment
                if (e == null || id != enrolmentId) return@finish
                if (take == null || take.issue != null) {
                    message = "Not used: ${describe(take?.issue)}. Try again."
                    screen = Screen.Recording(target, false)
                    return@finish
                }
                e.accept(target, take.features)
                message = "Kept."
                showNext()
            }
        }
    }

    private fun save() {
        val e = enrolment ?: return
        if (!e.canSave || coordinator.busy.value) return
        stopRecording()
        // Snapshot on the main thread: later edits cannot leak into this save.
        val id = enrolmentId
        val savePhrase = e.phrase
        val takes = e.phraseFeatures().map { it.copyOf() }
        val contrasts = e.similarFeatures().map { (label, f) -> ContrastTake(label, f.copyOf()) }
        var inconsistent: List<UInt> = emptyList()
        val started = coordinator.save(id, {
            val build = personalModel(savePhrase, takes, contrasts)
            inconsistent = build.inconsistent
            build.model
        }) { result ->
            result.onSuccess {
                enrolment?.clear()
                enrolment = null
                enrolmentId = coordinator.invalidate()
                phrase = savePhrase
                refreshSaved()
                message = if (inconsistent.isEmpty()) "Saved on this phone. Try your phrase."
                else "Saved. Recording ${inconsistent.joinToString { (it.toInt() + 1).toString() }} sounded different from the others; re-record it if tries look wrong."
                screen = Screen.Test
            }.onFailure {
                message = "Not saved: ${it.message ?: "please try again"}. Anything saved before is unchanged."
            }
        }
        if (started) message = "Saving…"
    }

    private fun cancelEnrolment() {
        // A commit in progress cannot be cancelled; Cancel is not offered then.
        if (coordinator.busy.value) return
        stopRecording()
        enrolment?.clear()
        enrolment = null
        enrolmentId = coordinator.invalidate()
        message = "Cancelled. Nothing new was saved."
        screen = Screen.Home
        refreshSaved()
    }

    private fun deleteSaved() {
        if (coordinator.busy.value) return
        stopRecording()
        enrolment?.clear()
        enrolment = null
        lastMatch = null
        coordinator.delete { result ->
            message = result.fold({ "Deleted from this phone." }, { "Could not delete everything: ${it.message}. Try again." })
            enrolmentId = coordinator.current()
            refreshSaved()
        }
        screen = Screen.Home
    }

    // ---- Trying the phrase ----

    private fun tryPhrase() {
        if (coordinator.busy.value) return
        stopRecording()
        val attempt = attempts.begin()
        val id = coordinator.current()
        val forPhrase = phrase
        lastMatch = null
        screen = Screen.Recording(null, false)
        withMicrophone { request ->
            record(request, PhraseEndpointer.TEST_MAX_MS) { audio ->
                message = "Comparing…"
                coordinator.withStore(id, { store ->
                    try {
                        runCatching {
                            val model = store.load() ?: throw IllegalStateException("Nothing saved to try")
                            personalMatch(model, forPhrase, audio, RATE.toUInt())
                        }
                    } finally {
                        audio.fill(0)
                    }
                }) { result ->
                    attempts.finish(attempt) {
                        screen = Screen.Test
                        result.onSuccess { lastMatch = it; message = "" }
                            .onFailure { message = it.message ?: "Could not compare" }
                    }
                }
            }
        }
    }

    // ---- UI ----

    @Composable
    private fun Lab() {
        @Suppress("UNUSED_EXPRESSION") revision
        val recording = screen as? Screen.Recording
        val base by NotdotState.orb.collectAsState()
        val orb = base.copy(
            mode = if (recording?.listening == true) OrbMode.LISTENING else OrbMode.IDLE,
            level = level, wakeReady = recording != null,
        )
        Column(
            Modifier.fillMaxSize().background(Color(0xFF050A14)).padding(16.dp).verticalScroll(rememberScrollState()),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text("Personal wake phrase (experiment)", color = Color.White, style = MaterialTheme.typography.titleLarge)
            Text(
                "Everything stays on this phone. Each recording is turned into a sound pattern straight away and the " +
                    "recording itself is thrown away. Only the patterns are kept, encrypted, and only when you tap Save. " +
                    "This is a test: it does not turn on wake listening, and it cannot tell who is speaking.",
                color = Color(0xFFB4C5D5), style = MaterialTheme.typography.bodyMedium,
            )
            if (recording != null) Orb(orb, Modifier.fillMaxWidth().height(200.dp))
            if (message.isNotEmpty()) Text(message, color = Color.White)
            when (val s = screen) {
                Screen.Home -> Home()
                is Screen.Recording -> Recording(s)
                Screen.Review -> Review()
                Screen.Test -> TryIt()
            }
        }
    }

    @Composable
    private fun ColumnScope.Wide(text: String, enabled: Boolean = true, primary: Boolean = false, onClick: () -> Unit) {
        val modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)
        if (primary) Button(onClick, modifier, enabled = enabled) { Text(text) }
        else OutlinedButton(onClick, modifier, enabled = enabled) { Text(text) }
    }

    @Composable
    private fun Home() {
        val busy by coordinator.busy.collectAsState()
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedTextField(phrase, { phrase = it.take(60); refreshSaved() }, Modifier.fillMaxWidth(),
                label = { Text("Wake phrase") }, singleLine = true)
            OutlinedTextField(similar, { similar = it.take(120) }, Modifier.fillMaxWidth(),
                label = { Text("Similar phrases to tell apart (optional, separated by commas)") })
            Text(saved ?: "Nothing saved.", color = Color(0xFFB4C5D5))
            Wide(if (saved == null) "Record my phrase" else "Record again", enabled = !busy, primary = true, onClick = ::startEnrolment)
            if (saved != null) {
                Wide("Try my phrase", enabled = !busy) { screen = Screen.Test }
                Wide("Delete saved recordings", enabled = !busy, onClick = ::deleteSaved)
            }
        }
    }

    @Composable
    private fun Recording(s: Screen.Recording) {
        val busy by coordinator.busy.collectAsState()
        val pending by attempts.pending.collectAsState()
        val prompt = when (val t = s.target) {
            is PersonalEnrolment.Target.Phrase ->
                "Say “${enrolment?.phrase}” (${t.index + 1} of ${PersonalEnrolment.PHRASE_TAKES}). Vary your tone and distance a little each time."
            is PersonalEnrolment.Target.Similar -> "Optional: say “${t.label}” once, so it can be told apart from your phrase."
            else -> "Say your phrase, or something else, once."
        }
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(prompt, color = Color.White, style = MaterialTheme.typography.titleMedium)
            Text(if (s.listening) "Hearing you…" else if (recorder != null) "Listening…" else if (pending) "Checking…" else "",
                color = Color(0xFFB4C5D5))
            if (!pending) {
                Wide("Record", enabled = !busy, primary = true) { s.target?.let(::recordTarget) ?: tryPhrase() }
            }
            if (s.target is PersonalEnrolment.Target.Similar) {
                Wide("Skip similar phrases") { stopRecording(); enrolment?.skipRemainingSimilar(); showNext() }
            }
            Wide("Cancel", enabled = !busy) { if (s.target == null) { stopRecording(); screen = Screen.Test } else cancelEnrolment() }
        }
    }

    @Composable
    private fun Review() {
        val e = enrolment ?: return
        val busy by coordinator.busy.collectAsState()
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("Recordings: ${e.phraseCount} of ${PersonalEnrolment.PHRASE_TAKES}", color = Color.White)
            for (i in 0 until PersonalEnrolment.PHRASE_TAKES) {
                Wide("Re-record ${i + 1}", enabled = !busy) { e.redo(PersonalEnrolment.Target.Phrase(i)); showNext() }
            }
            for (label in e.similarLabels) {
                Wide((if (e.hasSimilar(label)) "Re-record “" else "Record “") + label + "”", enabled = !busy) {
                    e.redo(PersonalEnrolment.Target.Similar(label)); showNext()
                }
            }
            Wide(if (busy) "Saving…" else "Save", enabled = e.canSave && !busy, primary = true, onClick = ::save)
            Wide("Cancel", enabled = !busy, onClick = ::cancelEnrolment)
        }
    }

    @Composable
    private fun TryIt() {
        val busy by coordinator.busy.collectAsState()
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(saved ?: "Nothing saved.", color = Color(0xFFB4C5D5))
            Wide("Try my phrase", enabled = !busy && saved != null, primary = true, onClick = ::tryPhrase)
            lastMatch?.let { m ->
                val headline = when {
                    m.issue != null -> "Not compared: ${describe(m.issue)}."
                    m.matched -> "Sounded like your phrase."
                    !m.withinBound -> "Did not sound like your phrase."
                    else -> "Sounded closer to “${m.contrastLabel}” than to your phrase."
                }
                Text(headline, color = Color.White, style = MaterialTheme.typography.titleMedium)
                Text("One try proves nothing on its own. Wake listening is unchanged.", color = Color(0xFFB4C5D5))
                TextButton({ showDetails = !showDetails }) { Text(if (showDetails) "Hide details" else "Details") }
                if (showDetails && m.issue == null) {
                    val near = m.nearestPhrase
                    Text("Distance to your recordings: %.3f (your recordings' own spread allows up to %.3f)".format(near ?: 0f, m.phraseBound),
                        color = Color(0xFFB4C5D5))
                    m.nearestContrast?.let {
                        Text("Distance to “${m.contrastLabel}”: %.3f".format(it), color = Color(0xFFB4C5D5))
                    } ?: Text("No similar phrases recorded.", color = Color(0xFFB4C5D5))
                    Text("Smaller is closer; 0 is identical.", color = Color(0xFFB4C5D5))
                }
            }
            Wide("Done") { stopRecording(); lastMatch = null; screen = Screen.Home }
        }
    }

    private fun describe(issue: TakeIssue?) = when (issue) {
        TakeIssue.NO_SPEECH -> "no speech heard"
        TakeIssue.TOO_QUIET -> "too quiet"
        TakeIssue.CLIPPED -> "too loud (distorted)"
        TakeIssue.TOO_SHORT -> "too short"
        TakeIssue.TOO_LONG -> "too long"
        TakeIssue.NOISY -> "too much background noise"
        null -> "it could not be processed"
    }

    companion object {
        const val RATE = 16_000
    }
}

/**
 * AES-GCM with a Keystore key (no user authentication), as IdentityStore.
 * Opening never creates a key; deleting reports failure.
 */
private class KeystoreSealer(private val alias: String = "notdot-personal-wake") : Sealer {
    private fun keystore() = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    private fun existing(): SecretKey? = keystore().getKey(alias, null) as SecretKey?

    private fun keyForSealing(): SecretKey = existing() ?: KeyGenerator
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

    override fun seal(name: String, data: ByteArray): ByteArray {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE, keyForSealing()) }
        cipher.updateAAD(name.toByteArray(Charsets.UTF_8))
        val body = cipher.doFinal(data)
        return byteArrayOf(cipher.iv.size.toByte()) + cipher.iv + body
    }

    override fun open(name: String, sealed: ByteArray): ByteArray {
        val key = existing() ?: throw GeneralSecurityException("the sealing key is missing")
        val ivSize = sealed.firstOrNull()?.toInt() ?: throw GeneralSecurityException("empty")
        if (ivSize !in 12..16 || sealed.size <= 1 + ivSize) throw GeneralSecurityException("malformed")
        val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply {
            init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, sealed, 1, ivSize))
        }
        cipher.updateAAD(name.toByteArray(Charsets.UTF_8))
        return cipher.doFinal(sealed, 1 + ivSize, sealed.size - 1 - ivSize)
    }

    override fun destroyKey() {
        val store = keystore()
        if (store.containsAlias(alias)) store.deleteEntry(alias)
        if (keystore().containsAlias(alias)) throw GeneralSecurityException("the sealing key could not be removed")
    }
}
