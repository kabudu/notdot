package dev.notdot.mobile

import android.app.Activity
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.testTag
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.notdot_mobile_core.MobileException
import uniffi.notdot_mobile_core.PhoneLink
import uniffi.notdot_mobile_core.StepUpProtection
import uniffi.notdot_mobile_core.StepUpStatus
import uniffi.notdot_mobile_core.describeProtection
import uniffi.notdot_mobile_core.newRequestId

/**
 * Sensitive actions from this phone: setting up the step-up key, and an
 * action the owner confirms with a fingerprint or face. These are settings
 * and approval controls; conversation stays voice-only.
 */
interface SensitiveActionsBackend {
    suspend fun status(): StepUpStatus
    suspend fun setUp(): StepUpProtection
    suspend fun resetMemory(requestId: String): ULong
}

class CoreSensitiveActions(
    private val activity: Activity,
    private val link: () -> PhoneLink,
) : SensitiveActionsBackend {
    override suspend fun status(): StepUpStatus {
        val status = link().stepUpStatus()
        withContext(Dispatchers.IO) { StepUpKeys.reconcile(activity, status.keySha256) }
        return status
    }

    override suspend fun setUp(): StepUpProtection {
        val attester = AndroidAttester(activity)
        return try {
            link().setUpStepUp(attester).also { withContext(Dispatchers.IO) { attester.finish(null) } }
        } catch (e: Exception) {
            withContext(Dispatchers.IO) { attester.finish(e) }
            throw e
        }
    }

    override suspend fun resetMemory(requestId: String) = link().resetMemory(
        requestId,
        StepUpKeys.signer(activity, "Erase assistant memories?", "Confirm to erase this profile's memories on your computer"),
    )
}

/** The outcome of the last attempt, in plain words. */
sealed class SensitiveOutcome(val text: String, val problem: Boolean = true) {
    class Done(text: String) : SensitiveOutcome(text, problem = false)
    data object Cancelled : SensitiveOutcome("Cancelled. Nothing changed.")
    class Denied(text: String) : SensitiveOutcome(text)
    data object Expired : SensitiveOutcome("That took too long, so nothing changed. Try again.")
    class Offline(text: String) : SensitiveOutcome("$text Try again when your computer is reachable; it won't be done twice.")
    class Revoked(text: String) : SensitiveOutcome(text)
    class Unavailable(text: String) : SensitiveOutcome(text)
    class Failed(text: String) : SensitiveOutcome("Something went wrong: $text")

    companion object {
        fun from(e: Exception): SensitiveOutcome = when (e) {
            is MobileException.Cancelled -> Cancelled
            is MobileException.Denied -> Denied(e.detail)
            is MobileException.Expired -> Expired
            is MobileException.Offline -> Offline(e.detail)
            is MobileException.Unpaired -> Revoked(e.detail)
            is MobileException.Unavailable -> Unavailable(e.detail)
            is MobileException -> Failed(e.message ?: "error")
            else -> Failed(e.message ?: e.javaClass.simpleName)
        }
    }
}

class SensitiveActionsModel(
    private val backend: SensitiveActionsBackend,
    private val onUnpaired: suspend (String) -> Unit,
) {
    private val _status = MutableStateFlow<StepUpStatus?>(null)
    val status: StateFlow<StepUpStatus?> = _status
    private val _working = MutableStateFlow<String?>(null)
    val working: StateFlow<String?> = _working
    private val _outcome = MutableStateFlow<SensitiveOutcome?>(null)
    val outcome: StateFlow<SensitiveOutcome?> = _outcome

    /** Kept after a dropped connection so a retry can never erase twice. */
    var pendingResetId: String? = null
        private set

    val busy get() = _working.value != null

    fun statusText(status: StepUpStatus?): String {
        if (status == null) return "Checking with your computer…"
        val protection = status.protection
        if (!status.enrolled || protection == null) {
            return "Not set up. Set up to confirm sensitive actions with a fingerprint or face."
        }
        val verified = describeProtection(protection)
        val test = when {
            status.testOnly -> " (test key, not real hardware)"
            status.development -> " (development build)"
            else -> ""
        }
        return if (status.granted) "Ready. $verified$test"
        else "Set up. $verified$test Allow sensitive actions for this phone on your computer to use them."
    }

    fun canAct(status: StepUpStatus?) = !busy && status?.enrolled == true && status.granted

    suspend fun refresh() {
        try {
            _status.value = backend.status()
        } catch (e: Exception) {
            finish(e)
        }
    }

    // Main-thread only (Compose callbacks), so the busy check and the
    // state change below cannot interleave.
    suspend fun setUp() {
        if (busy) return
        _working.value = "Setting up… this can take a few seconds."
        _outcome.value = null
        try {
            val protection = backend.setUp()
            _working.value = null
            _outcome.value = SensitiveOutcome.Done("Set up. ${describeProtection(protection)}")
            refresh()
        } catch (e: Exception) {
            finish(e)
        }
    }

    /** Called only after the owner confirms the destructive dialog. */
    suspend fun eraseMemories() {
        if (busy) return
        val id = pendingResetId ?: newRequestId()
        pendingResetId = id
        _working.value = "Waiting for your fingerprint or face…"
        _outcome.value = null
        try {
            val removed = backend.resetMemory(id)
            pendingResetId = null
            _working.value = null
            _outcome.value = SensitiveOutcome.Done(if (removed == 1UL) "Erased 1 memory." else "Erased $removed memories.")
        } catch (e: Exception) {
            finish(e)
        }
    }

    private suspend fun finish(e: Exception) {
        _working.value = null
        val outcome = SensitiveOutcome.from(e)
        // Keep the id only when the computer may have done it already.
        if (outcome !is SensitiveOutcome.Offline) pendingResetId = null
        _outcome.value = outcome
        if (outcome is SensitiveOutcome.Revoked) {
            _status.value = null
            onUnpaired(outcome.text)
        }
    }
}

/**
 * Debug-only scripted backend for UI tests and visual QA, chosen with the
 * `notdotSensitiveScenario` intent extra in debug builds. It never talks to
 * a computer.
 */
class ScriptedSensitiveActions(private val scenario: String) : SensitiveActionsBackend {
    private var enrolled = scenario !in setOf("setup", "unavailable")
    private val removed = mutableMapOf<String, ULong>()
    var effects = 0
        private set

    override suspend fun status() = StepUpStatus(
        enrolled = enrolled,
        protection = if (enrolled) StepUpProtection.ANDROID_STRONG_BOX else null,
        granted = enrolled && scenario != "notgranted",
        testOnly = true,
        development = false,
        keySha256 = null,
    )

    override suspend fun setUp(): StepUpProtection {
        delay(300)
        if (scenario == "unavailable") {
            throw MobileException.Unavailable("Set a screen lock and add a fingerprint or face to confirm sensitive actions.")
        }
        enrolled = true
        return StepUpProtection.ANDROID_STRONG_BOX
    }

    override suspend fun resetMemory(requestId: String): ULong {
        delay(300)
        removed[requestId]?.let { return it }
        when (scenario) {
            "cancelled" -> throw MobileException.Cancelled("cancelled")
            "denied", "notgranted" -> throw MobileException.Denied(
                "This phone isn't allowed to do that right now. Allow sensitive actions for it on your computer, then try again.")
            "expired" -> throw MobileException.Expired("expired")
            "offline" -> throw MobileException.Offline("Not connected to the computer.")
            "revoked" -> throw MobileException.Unpaired("The computer no longer accepts this phone. Link it again to continue.")
        }
        effects += 1
        removed[requestId] = 3UL
        return 3UL
    }
}

@Composable
fun SensitiveActionsScreen(model: SensitiveActionsModel) {
    val scope = rememberCoroutineScope()
    val status by model.status.collectAsState()
    val working by model.working.collectAsState()
    val outcome by model.outcome.collectAsState()
    var confirming by remember { mutableStateOf(false) }
    LaunchedEffect(model) { model.refresh() }
    val text = MaterialTheme.typography

    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text("Sensitive actions", color = Color.White, style = text.titleLarge, modifier = Modifier.semantics { heading() })
        Text(model.statusText(status), color = Color.White, style = text.bodyLarge, modifier = Modifier.semantics { testTag = "sensitiveStatus" })
        // Progress and results sit right under the status, so they stay on
        // screen at large text sizes.
        working?.let {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
                modifier = Modifier.semantics(mergeDescendants = true) { testTag = "working" }) {
                CircularProgressIndicator(Modifier.padding(4.dp))
                Text(it, color = Color.White, style = text.bodyLarge)
            }
        }
        outcome?.let {
            Text(
                (if (it.problem) "⚠ " else "✓ ") + it.text,
                color = if (it.problem) Color(0xFFFFB74D) else Color(0xFF81C784),
                style = text.bodyLarge,
                modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite; testTag = "outcome" },
            )
        }
        Text(
            "Sensitive actions change things on your computer. Each one needs your fingerprint or face, and your computer must allow this phone first.",
            color = Color(0xFFC9D1DC), style = text.bodyMedium,
        )
        if (status?.enrolled != true) {
            Button(
                { scope.launch { model.setUp() } },
                Modifier.heightIn(min = 48.dp).semantics { testTag = "setUp" },
                enabled = working == null && status != null,
            ) { Text("Set up sensitive actions") }
        }
        Button(
            { confirming = true },
            Modifier.heightIn(min = 48.dp).semantics { testTag = "erase" },
            enabled = working == null && model.canAct(status),
            colors = ButtonDefaults.buttonColors(containerColor = Color(0xFFB3261E), contentColor = Color.White),
        ) { Text("Erase assistant memories") }
        Text(
            "Erases everything the assistant remembers for this profile on your computer. Other profiles aren't affected.",
            color = Color(0xFFC9D1DC), style = text.bodyMedium,
        )
    }

    if (confirming) {
        AlertDialog(
            onDismissRequest = { confirming = false },
            title = { Text("Erase this profile's memories?") },
            text = {
                Text("The assistant on your computer will forget everything it remembers for this profile. This can't be undone. You'll confirm with your fingerprint or face.")
            },
            confirmButton = {
                TextButton({
                    confirming = false
                    scope.launch { model.eraseMemories() }
                }, colors = ButtonDefaults.textButtonColors(contentColor = Color(0xFFF2B8B5))) { Text("Erase memories") }
            },
            dismissButton = { TextButton({ confirming = false }) { Text("Keep memories") } },
        )
    }
}
