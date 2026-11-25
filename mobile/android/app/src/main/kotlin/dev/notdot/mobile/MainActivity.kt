package dev.notdot.mobile

import androidx.activity.compose.rememberLauncherForActivityResult
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions

import android.Manifest
import android.content.pm.PackageManager
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.notdot_mobile_core.MobileException
import uniffi.notdot_mobile_core.PairResult
import uniffi.notdot_mobile_core.LinkRoute
import uniffi.notdot_mobile_core.PhoneLink
import uniffi.notdot_mobile_core.SessionEvent

/**
 * Voice-only home: orb, one status line and Tap to talk (the default way to
 * speak to the paired computer). Hands-free wake listening is a separate,
 * opt-in experiment. Text only in setup.
 */
class MainActivity : ComponentActivity() {
    private val microphone = registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { granted ->
        if (granted[Manifest.permission.RECORD_AUDIO] == true) VoiceSessionService.start(this)
        else NotdotState.setStatus("Microphone access is off. Turn it on in Settings to talk hands-free.")
    }

    private val talkMicrophone = registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) talk() else NotdotState.setStatus("Microphone access is off. Turn it on in Settings to talk.")
    }

    private val notification = registerForActivityResult(ActivityResultContracts.RequestPermission()) {
        VoiceSessionService.start(this)
    }
    private fun openPermissions() {
        startActivity(android.content.Intent(android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
            android.net.Uri.parse("package:$packageName")))
    }
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Debug-only: a scripted sensitive-actions backend for UI tests.
        val scenario = if (BuildConfig.DEBUG) intent.getStringExtra("notdotSensitiveScenario") else null
        restoreLink()
        setContent {
            MaterialTheme(colorScheme = darkColorScheme()) { Home(::talk, ::startHandsFree, ::openPermissions, scenario) }
        }
    }

    /** The paired computer, for voice turns, from launch (Keystore-sealed; off the main thread). */
    private fun restoreLink() {
        if (NotdotState.link != null) return
        val store = IdentityStore.standard(this)
        kotlin.concurrent.thread(name = "notdot-link-restore") {
            val restored = runCatching { store.restoreLink() }.getOrNull() ?: return@thread
            val paired = runCatching { kotlinx.coroutines.runBlocking { restored.pairedRecord() } != null }.getOrDefault(false)
            synchronized(NotdotState) {
                if (NotdotState.link == null) {
                    NotdotState.link = restored
                    NotdotState.linked = paired
                    NotdotState.setStatus(NotdotState.idleText())
                }
            }
        }
    }

    /**
     * Tap to talk (user action, app visible). While hands-free is running,
     * a tap starts a request there instead (one microphone owner).
     */
    private fun talk() {
        if (NotdotState.silent) {
            NotdotState.setStatus("Silent test mode: microphone stays off")
            return
        }
        if (!permitted()) {
            talkMicrophone.launch(Manifest.permission.RECORD_AUDIO)
            return
        }
        if (NotdotState.effects.value.surface) {
            VoiceSessionService.talk(this)
            return
        }
        val tap = TalkPath.get()
        if (tap.state == TalkState.IDLE && !NotdotState.linked) {
            NotdotState.setStatus("Link this phone to your computer to talk.")
            return
        }
        if (!tap.tap()) NotdotState.setStatus("The microphone could not start. Please try again.")
    }

    override fun onStart() {
        super.onStart()
        NotdotState.handle(SessionEvent.AppVisible, permitted())
        // Debug-only unattended test hook: start a session from the visible
        // activity (RECORD_AUDIO must already be granted, e.g. `pm grant`).
        if (BuildConfig.DEBUG && intent.getBooleanExtra("notdotAutoStart", false) && permitted()) {
            intent.removeExtra("notdotAutoStart")
            startHandsFree()
        }
    }

    override fun onStop() {
        // Tap to talk only listens while the app is in use; a reply keeps playing.
        TalkPath.get().hidden()
        NotdotState.handle(SessionEvent.AppHidden, permitted())
        super.onStop()
    }

    private fun permitted() = checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED

    /** User action, app visible: allowed to start a microphone service. */
    private fun startHandsFree() {
        if (NotdotState.silent) {
            NotdotState.setStatus("Silent test mode: microphone stays off")
            return
        }
        if (permitted()) {
            if (android.os.Build.VERSION.SDK_INT >= 33 && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED)
                notification.launch(Manifest.permission.POST_NOTIFICATIONS)
            else VoiceSessionService.start(this)
        } else {
            val permissions = if (android.os.Build.VERSION.SDK_INT >= 33)
                arrayOf(Manifest.permission.RECORD_AUDIO, Manifest.permission.POST_NOTIFICATIONS)
            else arrayOf(Manifest.permission.RECORD_AUDIO)
            microphone.launch(permissions)
        }
    }
}

@Composable
private fun Home(talk: () -> Unit, start: () -> Unit, permissions: () -> Unit, scenario: String?) {
    val effects by NotdotState.effects.collectAsState()
    val talking by TalkPath.state.collectAsState()
    val paired by NotdotState.paired.collectAsState()
    val status by NotdotState.status.collectAsState()
    val orb by NotdotState.orb.collectAsState()
    var linking by remember { mutableStateOf(scenario != null) }
    Column(
        Modifier.fillMaxSize().background(Color(0xFF050A14)).padding(24.dp)
            .then(if (linking) Modifier.verticalScroll(rememberScrollState()) else Modifier),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        if (!linking) Orb(orb, Modifier.weight(1f).fillMaxWidth())
        Text(status, color = Color.White, style = MaterialTheme.typography.titleMedium)
        val context = androidx.compose.ui.platform.LocalContext.current
        if (!linking) Button({ if (paired) talk() else linking = true }, Modifier.heightIn(min = 56.dp).fillMaxWidth()) {
            Text(
                when (talking) {
                    TalkState.IDLE -> if (paired) "Tap to talk" else "Link this phone"
                    TalkState.LISTENING -> "Listening · tap to stop"
                    TalkState.REPLYING -> "Stop"
                },
            )
        }
        if (effects.surface) {
            OutlinedButton({ VoiceSessionService.stop(context) }, Modifier.heightIn(min = 48.dp)) { Text("Stop hands-free") }
        } else {
            OutlinedButton(start, Modifier.heightIn(min = 48.dp)) { Text("Hands-free (experimental)") }
        }
        OutlinedButton({ linking = !linking }, Modifier.heightIn(min = 48.dp)) { Text("Linked computer") }
        if (BuildConfig.DEBUG && BuildConfig.PERSONAL_WAKE_LAB && !linking) {
            OutlinedButton(
                { context.startActivity(android.content.Intent(context, PersonalWakeActivity::class.java)) },
                Modifier.heightIn(min = 48.dp),
            ) { Text("Personal wake phrase (experiment)") }
        }
        if (linking) {
            OutlinedButton(permissions, Modifier.heightIn(min = 48.dp)) { Text("App permissions") }
            Text("Choose While using the app for the microphone. Start hands-free to keep listening with a stoppable notification.", color = Color(0xFFB4C5D5), style = MaterialTheme.typography.bodyMedium)
            Pairing(scenario) {
                NotdotState.setStatus("Connected. Tap to talk.")
                linking = false
            }
        }
    }
}

@Composable
private fun Pairing(scenario: String?, onConnected: () -> Unit) {
    val scope = rememberCoroutineScope()
    val context = androidx.compose.ui.platform.LocalContext.current
    // Identity and paired computer survive restarts (Keystore-sealed).
    val store = remember { IdentityStore.standard(context) }
    var link by remember { mutableStateOf(NotdotState.link ?: runCatching { store.restoreLink() }.getOrElse { PhoneLink() }) }
    // Voice turns use whichever link this screen holds.
    LaunchedEffect(link) {
        NotdotState.link = link
        NotdotState.linked = link.pairedRecord() != null
    }
    var qr by remember { mutableStateOf("") }
    var message by remember { mutableStateOf("Not linked") }
    var showSensitive by remember { mutableStateOf(scenario != null) }
    val sensitive = remember {
        val backend: SensitiveActionsBackend = scenario?.let(::ScriptedSensitiveActions)
            ?: CoreSensitiveActions(context as android.app.Activity) { link }
        SensitiveActionsModel(backend) { detail ->
            // Removed at the computer: forget everything on this phone.
            withContext(Dispatchers.IO) {
                store.clear()
                StepUpKeys.clear(context)
            }
            link = PhoneLink()
            message = detail
        }
    }
    var pairing by remember { mutableStateOf(false) }
    var showPaste by remember { mutableStateOf(false) }
    fun pair(invitation: String) {
        if (pairing) return
        pairing = true
        message = "Connecting to your computer…"
        scope.launch {
            message = try {
                // The identity is created (once) by this user action.
                if (store.identity() == null) link = PhoneLink.restore(store.identityCreatingIfNeeded(), null)
                val code = link.pair(invitation.trim(), "Android phone")
                message = "Waiting for approval on your Mac. Compare this code: $code."
                when (val result = link.awaitApproval()) {
                    is PairResult.Approved -> {
                        store.savePairedRecord(link.pairedRecord())
                        NotdotState.linked = true
                        val route = link.connect(false)
                        if (route != LinkRoute.DISCONNECTED) onConnected()
                        "Linked ($route)"
                    }
                    PairResult.Denied -> "The computer said no."
                    is PairResult.Rejected -> result.reason
                }
            } catch (e: MobileException.Unpaired) {
                // Removed at the computer: forget everything on this phone.
                withContext(Dispatchers.IO) {
                    store.clear()
                    StepUpKeys.clear(context)
                }
                link = PhoneLink()
                e.detail
            } catch (e: Exception) {
                e.message ?: "Could not link"
            } finally {
                pairing = false
            }
        }
    }
    val scanner = rememberLauncherForActivityResult(ScanContract()) { result ->
        result.contents?.let { invitation ->
            if (invitation.startsWith("notdot-pair:1:")) pair(invitation)
            else message = "That is not a notdot pairing QR code."
        }
    }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Button({
            scanner.launch(ScanOptions().apply {
                setCaptureActivity(PairingCaptureActivity::class.java)
                setOrientationLocked(false)
                setDesiredBarcodeFormats(ScanOptions.QR_CODE)
                setPrompt("Fit the code on your computer inside the square")
                setBeepEnabled(false)
            })
        }, Modifier.heightIn(min = 48.dp), enabled = !pairing) { Text("Scan QR code") }
        if (showPaste) {
            OutlinedTextField(qr, { qr = it }, label = { Text("Paste pairing code") }, singleLine = true)
            OutlinedButton({ pair(qr) }, Modifier.heightIn(min = 48.dp), enabled = !pairing && qr.isNotBlank()) {
                Text("Link")
            }
        } else {
            OutlinedButton({ showPaste = true }, Modifier.heightIn(min = 48.dp)) { Text("Pair without a camera") }
        }
        Text(message, color = Color.White)
        OutlinedButton({ showSensitive = !showSensitive }, Modifier.heightIn(min = 48.dp)) { Text("Sensitive actions") }
        if (showSensitive) SensitiveActionsScreen(sensitive)
    }
}
