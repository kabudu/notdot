package dev.notdot.mobile

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import android.os.IBinder
import uniffi.notdot_mobile_core.MobileException
import uniffi.notdot_mobile_core.PhoneticListener
import uniffi.notdot_mobile_core.SessionEvent
import uniffi.notdot_mobile_core.StartSource
import uniffi.notdot_mobile_core.phoneticAvailable
import java.io.File
import kotlin.concurrent.thread
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel

/**
 * A user-started hands-free session: a `microphone` foreground service with
 * a visible, stoppable notification. It must be started while the app is
 * visible or from an exempt user action (notification, widget). It records;
 * replies play through [ReplyPlayer]. In silent mode it never opens the microphone.
 *
 * Wake: if this build has the phonetic detector and a phonetic enrolment is
 * saved on this phone, it listens continuously for that enrolled phrase (any
 * name); otherwise it uses the existing detector. Whichever is used, wake is
 * ignored while a reply plays and briefly after it ([PlaybackGate]).
 */
class VoiceSessionService : Service() {
    @Volatile private var capturing = false
    @Volatile private var recorder: AudioRecord? = null

    private val main = android.os.Handler(android.os.Looper.getMainLooper())
    private val restartToken = Any()
    @Volatile private var nativeWake: OnDeviceWakeRecognizer? = null
    @Volatile private var wakeReady = false
    private var restartPolicy = WakeRestartPolicy()
    private var listeningSince = 0L
    @Volatile private var microphoneSilenced = false
    @Volatile private var voiceTurns: VoiceTurns? = null
    private var voiceScope: CoroutineScope? = null
    private var languageIndex = 0
    private val playback = PlaybackGate({ android.os.SystemClock.elapsedRealtime() })
    private val nativeLanguages by lazy {
        (listOf(resources.configuration.locales[0].toLanguageTag()).filter { it.startsWith("en", true) } +
            listOf("en-GB", "en-US", "en")).distinct()
    }

    private fun startWake() {
        nativeWake?.stop()
        wakeReady = false
        listeningSince = 0
        val detector = OnDeviceWakeRecognizer(this, object : OnDeviceWakeRecognizer.Listener {
            override fun onStatus(status: WakeRecognizerStatus) {
                when (status) {
                    WakeRecognizerStatus.CheckingLanguage -> NotdotState.setStatus("Preparing on-device wake listening…")
                    is WakeRecognizerStatus.Listening -> {
                        wakeReady = true
                        listeningSince = android.os.SystemClock.elapsedRealtime()
                        NotdotState.setStatus(NotdotState.idleText())
                    }
                    is WakeRecognizerStatus.Ended -> {
                        wakeReady = false
                        scheduleWakeRestart()
                    }
                }
            }
            override fun onWake() {
                if (playback.suppressed()) return
                NotdotState.wakeHeard()
                if (NotdotState.link != null) voiceTurns?.wake()
            }
            override fun onError(error: WakeRecognizerError) {
                wakeReady = false
                val languageUnavailable = error is WakeRecognizerError.LanguageNotInstalled ||
                    (error is WakeRecognizerError.Recognizer && error.code in setOf(12, 13))
                if (languageUnavailable && languageIndex + 1 < nativeLanguages.size) {
                    languageIndex++
                    scheduleWakeRestart()
                } else if (WakeRestartPolicy.retryable(error)) {
                    scheduleWakeRestart()
                } else {
                    stop(SessionEvent.SystemStopped)
                    NotdotState.setStatus(when (error) {
                        is WakeRecognizerError.LanguageNotInstalled -> "On-device English recognition is not installed on this phone."
                        is WakeRecognizerError.Unsupported -> "On-device wake listening is unavailable on this phone."
                        is WakeRecognizerError.Recognizer -> if (error.code in setOf(12, 13)) "On-device English recognition is not installed for this language." else "Wake listening stopped. Please start hands-free again."
                        else -> "Wake listening stopped. Please start hands-free again."
                    })
                }
            }
        }, language = nativeLanguages[languageIndex])
        nativeWake = detector
        detector.start("Ava")
    }

    private fun scheduleWakeRestart() {
        if (!capturing) return
        val old = nativeWake
        val sessionMs = if (listeningSince == 0L) 0L else android.os.SystemClock.elapsedRealtime() - listeningSince
        val delay = restartPolicy.nextDelayMs(sessionMs)
        NotdotState.setStatus("Reconnecting on-device wake listening…")
        main.removeCallbacksAndMessages(restartToken)
        main.postDelayed({
            if (capturing && !microphoneSilenced && nativeWake === old) {
                old?.stop()
                startWake()
            }
        }, restartToken, delay)
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            stop(SessionEvent.Stop)
            return START_NOT_STICKY
        }
        if (intent?.action == ACTION_TALK) {
            // Tap to talk while hands-free owns the microphone: a request
            // starts here (the same as hearing the wake phrase). Never starts
            // a session by itself.
            if (capturing && !microphoneSilenced) {
                NotdotState.wakeHeard()
                voiceTurns?.wake()
            } else if (!capturing) stopSelf()
            return START_NOT_STICKY
        }
        val permitted = checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
        if (NotdotState.silent || !permitted) {
            NotdotState.setStatus(if (NotdotState.silent) "Silent test mode: microphone stays off" else "Microphone access is off")
            stopSelf()
            return START_NOT_STICKY
        }
        try {
            if (android.os.Build.VERSION.SDK_INT >= 30)
                startForeground(NOTIFICATION_ID, notification(), ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE)
            else startForeground(NOTIFICATION_ID, notification())
        } catch (_: IllegalStateException) {
            NotdotState.setStatus("Open notdot to start hands-free listening.")
            stopSelf()
            return START_NOT_STICKY
        } catch (_: SecurityException) {
            NotdotState.setStatus("Open notdot and check microphone access before starting hands-free.")
            stopSelf()
            return START_NOT_STICKY
        }
        val effects = NotdotState.handle(SessionEvent.Start(StartSource.APP), microphonePermitted = true)
        if (effects.microphone) startCapture()
        return START_NOT_STICKY
    }

    private fun notification(): Notification {
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, "Hands-free listening", NotificationManager.IMPORTANCE_LOW).apply { setSound(null, null) },
        )
        val stop = PendingIntent.getService(
            this, 0, Intent(this, VoiceSessionService::class.java).setAction(ACTION_STOP), PendingIntent.FLAG_IMMUTABLE,
        )
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
        return Notification.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.ic_btn_speak_now)
            .setContentTitle("notdot is listening")
            .setContentText("Say its name to talk. Stop any time.")
            .setContentIntent(open)
            .setOngoing(true)
            .addAction(Notification.Action.Builder(null, "Stop", stop).build())
            .build()
    }

    @Suppress("MissingPermission")
    private fun startCapture() {
        if (capturing) return
        val rate = 16_000
        val size = AudioRecord.getMinBufferSize(rate, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        if (size <= 0) {
            stop(SessionEvent.SystemStopped)
            NotdotState.setStatus("This microphone cannot provide wake audio.")
            return
        }
        val record = try { AudioRecord(
            MediaRecorder.AudioSource.VOICE_RECOGNITION, rate, AudioFormat.CHANNEL_IN_MONO,
            AudioFormat.ENCODING_PCM_16BIT, maxOf(size, rate * 2 / 2),
        ) } catch (_: RuntimeException) {
            stop(SessionEvent.SystemStopped)
            NotdotState.setStatus("Microphone capture could not start.")
            return
        }
        recorder = record
        capturing = true
        try {
            check(record.state == AudioRecord.STATE_INITIALIZED)
            record.startRecording()
            check(record.recordingState == AudioRecord.RECORDSTATE_RECORDING)
        } catch (_: RuntimeException) {
            stop(SessionEvent.SystemStopped)
            NotdotState.setStatus("Microphone capture could not start.")
            return
        }
        restartPolicy = WakeRestartPolicy()
        listeningSince = 0
        languageIndex = 0
        microphoneSilenced = false
        NotdotState.setStatus("Preparing wake listening…")
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
        voiceScope = scope
        voiceTurns = VoiceTurns(PhoneLinkClient { NotdotState.link }, ReplyPlayer { playback.playback(it) }, scope) { NotdotState.voice(it) }
        thread(name = "notdot-capture") {
            val packet = ByteArray(rate / 50 * 2)
            var meterSum = 0f
            var meterCount = 0
            var meterPeak = 0f
            val phonetic = phoneticListener()
            val native = phonetic == null && BuildConfig.NATIVE_WAKE
            if (phonetic != null) {
                wakeReady = true
                NotdotState.wakePhrase = displayPhrase(phonetic.phrase())
                NotdotState.setStatus(NotdotState.idleText())
            } else if (native) {
                NotdotState.wakePhrase = "Hey Ava"
                main.post { if (capturing && recorder === record) startWake() }
            }
            val legacy = if (phonetic != null || native) null else WakeModels.engine(this)
            if (phonetic == null && !native && legacy == null) {
                failCapture(record, "Wake listening could not start. Please try again.")
                return@thread
            }
            legacy?.reset()
            if (legacy != null) NotdotState.wakePhrase = "Hey Ava"
            var silenced = false
            while (capturing && recorder === record) {
                val read = try { record.read(packet, 0, packet.size, AudioRecord.READ_BLOCKING) }
                catch (_: IllegalStateException) { -1 }
                if (!capturing || recorder !== record) break
                if (read <= 0) {
                    failCapture(record, "Microphone capture stopped. Please start hands-free again.")
                    break
                }
                // Android gives silenced apps zeros while a call or assistant
                // holds the microphone; the session shows "Paused".
                val nowSilenced = record.activeRecordingConfiguration?.isClientSilenced == true
                if (nowSilenced != silenced) {
                    silenced = nowSilenced
                    microphoneSilenced = silenced
                    val changedToSilenced = silenced
                    if (!changedToSilenced) phonetic?.reset()
                    if (native) main.post {
                        if (capturing && recorder === record) {
                            if (changedToSilenced) {
                                main.removeCallbacksAndMessages(restartToken)
                                nativeWake?.stop()
                                wakeReady = false
                            } else startWake()
                        }
                    }
                    NotdotState.handle(SessionEvent.Silenced(silenced))
                }
                if (silenced) continue
                if (legacy != null) wakeReady = legacy.backupState() == uniffi.notdot_mobile_core.BackupState.READY
                // Level only (never audio) for diagnostics, once a second.
                var sum = 0.0
                for (i in 0 until read - 1 step 2) {
                    val sample = (((packet[i + 1].toInt() shl 8) or (packet[i].toInt() and 0xff)).toShort()).toDouble() / 32768.0
                    sum += sample * sample
                }
                val rms = kotlin.math.sqrt(sum / (read / 2).coerceAtLeast(1)).toFloat()
                if (wakeReady) NotdotState.wakeInput(rms, legacy?.backupBusy() == true)
                meterSum += rms; meterCount++; meterPeak = maxOf(meterPeak, rms)
                if (meterCount * read >= rate * 2) {
                    val effects = NotdotState.handle(SessionEvent.Tick)
                    if (effects.endSurface) {
                        main.post { if (recorder === record) stop(SessionEvent.SystemStopped) }
                        break
                    }
                    android.util.Log.i("notdot.voice", "capture ready=$wakeReady mean_rms=${meterSum / meterCount} peak_rms=$meterPeak")
                    meterSum = 0f; meterCount = 0; meterPeak = 0f
                }
                val bytes = if (read == packet.size) packet else packet.copyOf(read)
                val detector = nativeWake
                // The phone's own reply (and its echo) never wakes it; a
                // detector restarts afterwards rather than joining audio
                // from either side of the reply.
                val muted = playback.suppressed()
                if (!muted && playback.resumed()) {
                    phonetic?.reset()
                    legacy?.reset()
                }
                if (capturing && recorder === record) {
                    // The native recogniser keeps its session; its wakes are ignored while muted.
                    if (native) detector?.feedPcm16(bytes)
                    else if (!muted) {
                        val woke = if (phonetic != null) {
                            try { phonetic.pushPcm16(bytes) } catch (_: MobileException) { phonetic.reset(); false }
                        } else legacy?.acceptPcm16(bytes, rate.toUInt()) == true
                        if (woke && capturing && recorder === record) {
                            NotdotState.wakeHeard()
                            if (NotdotState.link != null) voiceTurns?.wake()
                        }
                    }
                    // After the wake phrase, the same audio is the request.
                    voiceTurns?.takeIf { it.listening }?.feed(bytes)
                }
            }
        }
    }

    /**
     * The continuous phonetic listener for the enrolment saved on this
     * phone (read only; the enrolment is never changed here), or null if
     * this build lacks the detector, nothing is saved, or it can't be used.
     */
    private fun phoneticListener(): PhoneticListener? {
        if (!phoneticAvailable()) return null
        val d = LabDetector.PHONETIC
        return runCatching {
            val model = PersonalWakeStore(File(noBackupFilesDir, d.folder), KeystoreSealer(d.alias), d.maxBytes).load()
                ?: return null
            PhoneticListener(WakeModels.directory(this).absolutePath, model)
        }.onFailure { android.util.Log.w("notdot.voice", "phonetic wake unavailable: ${it.javaClass.simpleName}") }
            .getOrNull()
    }

    private fun failCapture(record: AudioRecord, message: String) {
        main.post {
            // A late failure from an older capture never stops a new session.
            if (recorder === record) {
                stop(SessionEvent.SystemStopped)
                NotdotState.setStatus(message)
            }
        }
    }

    private fun stop(event: SessionEvent) {
        capturing = false
        voiceTurns?.stop()
        voiceTurns = null
        voiceScope?.cancel()
        voiceScope = null
        main.removeCallbacksAndMessages(null)
        wakeReady = false
        nativeWake?.stop()
        nativeWake = null
        val old = recorder
        recorder = null
        old?.run {
            runCatching { stop() }
            release()
        }
        NotdotState.handle(event)
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    override fun onDestroy() {
        if (capturing) stop(SessionEvent.SystemStopped)
        super.onDestroy()
    }

    companion object {
        private const val CHANNEL = "hands-free"
        private const val NOTIFICATION_ID = 1
        const val ACTION_STOP = "dev.notdot.mobile.STOP"
        const val ACTION_TALK = "dev.notdot.mobile.TALK"

        fun start(context: Context) {
            context.startForegroundService(Intent(context, VoiceSessionService::class.java))
        }

        /** Tap to talk while hands-free is running (see [ACTION_TALK]). */
        fun talk(context: Context) {
            context.startService(Intent(context, VoiceSessionService::class.java).setAction(ACTION_TALK))
        }

        fun stop(context: Context) {
            context.startService(Intent(context, VoiceSessionService::class.java).setAction(ACTION_STOP))
        }
    }
}
