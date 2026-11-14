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
import uniffi.notdot_mobile_core.SessionEvent
import uniffi.notdot_mobile_core.StartSource
import kotlin.concurrent.thread

/**
 * A user-started hands-free session: a `microphone` foreground service with
 * a visible, stoppable notification. It must be started while the app is
 * visible or from an exempt user action (notification, widget). It records;
 * it never plays audio. In silent mode it never opens the microphone.
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
    private var languageIndex = 0
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
                        NotdotState.setStatus("Listening hands-free · Say Hey Ava")
                    }
                    is WakeRecognizerStatus.Ended -> {
                        wakeReady = false
                        scheduleWakeRestart()
                    }
                }
            }
            override fun onWake() {
                NotdotState.wakeHeard()
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
            AudioFormat.ENCODING_PCM_16BIT, maxOf(size, rate / 50 * 2 * 4),
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
        if (BuildConfig.NATIVE_WAKE) startWake()
        thread(name = "notdot-capture") {
            val packet = ByteArray(rate / 50 * 2)
            var meterSum = 0f
            var meterCount = 0
            var meterPeak = 0f
            val legacy = if (BuildConfig.NATIVE_WAKE) null else WakeModels.engine(this)
            if (!BuildConfig.NATIVE_WAKE && legacy == null) {
                failCapture(record, "Wake listening could not start. Please try again.")
                return@thread
            }
            legacy?.reset()
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
                    if (BuildConfig.NATIVE_WAKE) main.post {
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
                if (!BuildConfig.NATIVE_WAKE) wakeReady = legacy?.backupState() == uniffi.notdot_mobile_core.BackupState.READY
                // Level only (never audio) for diagnostics, once a second.
                var sum = 0.0
                for (i in 0 until read - 1 step 2) {
                    val sample = (((packet[i + 1].toInt() shl 8) or (packet[i].toInt() and 0xff)).toShort()).toDouble() / 32768.0
                    sum += sample * sample
                }
                val rms = kotlin.math.sqrt(sum / (read / 2).coerceAtLeast(1)).toFloat()
                if (wakeReady) NotdotState.wakeInput(rms, !BuildConfig.NATIVE_WAKE && legacy?.backupBusy() == true)
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
                if (capturing && recorder === record) {
                    if (BuildConfig.NATIVE_WAKE) detector?.feedPcm16(bytes)
                    else {
                        val woke = legacy?.acceptPcm16(bytes, rate.toUInt()) == true
                        if (woke && capturing && recorder === record) NotdotState.wakeHeard()
                    }
                }
            }
        }
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

        fun start(context: Context) {
            context.startForegroundService(Intent(context, VoiceSessionService::class.java))
        }

        fun stop(context: Context) {
            context.startService(Intent(context, VoiceSessionService::class.java).setAction(ACTION_STOP))
        }
    }
}
