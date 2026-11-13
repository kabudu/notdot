package dev.notdot.mobile

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
import uniffi.notdot_mobile_core.PhoneLink
import uniffi.notdot_mobile_core.SessionEvent

/** Voice-only home: orb, one status line, Start/Stop. Text only in setup. */
class MainActivity : ComponentActivity() {
    private val microphone = registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { granted ->
        if (granted[Manifest.permission.RECORD_AUDIO] == true) VoiceSessionService.start(this)
        else NotdotState.setStatus("Microphone access is off. Turn it on in Settings to talk hands-free.")
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Debug-only: a scripted sensitive-actions backend for UI tests.
        val scenario = if (BuildConfig.DEBUG) intent.getStringExtra("notdotSensitiveScenario") else null
        setContent {
            MaterialTheme(colorScheme = darkColorScheme()) { Home(::startHandsFree, scenario) }
        }
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
        if (permitted()) VoiceSessionService.start(this)
        else microphone.launch(arrayOf(Manifest.permission.RECORD_AUDIO, Manifest.permission.POST_NOTIFICATIONS))
    }
}

@Composable
private fun Home(start: () -> Unit, scenario: String?) {
    val effects by NotdotState.effects.collectAsState()
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
        if (effects.surface) {
            Button({ VoiceSessionService.stop(context) }, Modifier.heightIn(min = 48.dp)) { Text("Stop listening") }
        } else {
            Button(start, Modifier.heightIn(min = 48.dp)) { Text("Start hands-free") }
        }
        OutlinedButton({ linking = !linking }, Modifier.heightIn(min = 48.dp)) { Text("Linked computer") }
        if (linking) Pairing(scenario)
    }
}

@Composable
private fun Pairing(scenario: String?) {
    val scope = rememberCoroutineScope()
    val context = androidx.compose.ui.platform.LocalContext.current
    // Identity and paired computer survive restarts (Keystore-sealed).
    val store = remember { IdentityStore.standard(context) }
    var link by remember { mutableStateOf(runCatching { store.restoreLink() }.getOrElse { PhoneLink() }) }
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
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        OutlinedTextField(qr, { qr = it }, label = { Text("Pairing code from the computer") }, singleLine = true)
        Button({
            scope.launch {
                message = try {
                    // The identity is created (once) by this user action.
                    if (store.identity() == null) link = PhoneLink.restore(store.identityCreatingIfNeeded(), null)
                    val code = link.pair(qr.trim(), "Android phone")
                    message = "Check the computer shows $code, then choose Allow there."
                    when (val result = link.awaitApproval()) {
                        is PairResult.Approved -> {
                            store.savePairedRecord(link.pairedRecord())
                            "Linked (${link.connect(false)})"
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
                }
            }
        }, Modifier.heightIn(min = 48.dp), enabled = qr.isNotBlank()) { Text("Link") }
        Text(message, color = Color.White)
        OutlinedButton({ showSensitive = !showSensitive }, Modifier.heightIn(min = 48.dp)) { Text("Sensitive actions") }
        if (showSensitive) SensitiveActionsScreen(sensitive)
    }
}
