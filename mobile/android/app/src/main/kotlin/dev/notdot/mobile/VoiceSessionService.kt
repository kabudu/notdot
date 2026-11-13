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
    private var recorder: AudioRecord? = null

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
        startForeground(NOTIFICATION_ID, notification(), ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE)
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
        val size = AudioRecord.getMinBufferSize(rate, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_FLOAT)
        val record = AudioRecord(
            MediaRecorder.AudioSource.VOICE_RECOGNITION, rate, AudioFormat.CHANNEL_IN_MONO,
            AudioFormat.ENCODING_PCM_FLOAT, maxOf(size, rate / 50 * 4 * 4),
        )
        recorder = record
        capturing = true
        record.startRecording()
        thread(name = "notdot-capture") {
            val packet = FloatArray(rate / 50)
            var meterSum = 0f
            var meterCount = 0
            var meterPeak = 0f
            NotdotState.setStatus("Preparing wake listening…")
            val wake = WakeModels.engine(this)
            var announcedReady = false
            var silenced = false
            while (capturing) {
                val read = record.read(packet, 0, packet.size, AudioRecord.READ_BLOCKING)
                if (read <= 0) continue
                // Android gives silenced apps zeros while a call or assistant
                // holds the microphone; the session shows "Paused".
                val nowSilenced = record.activeRecordingConfiguration?.isClientSilenced == true
                if (nowSilenced != silenced) {
                    silenced = nowSilenced
                    NotdotState.handle(SessionEvent.Silenced(silenced))
                }
                if (silenced) continue
                if (!announcedReady && wake?.backupState() == uniffi.notdot_mobile_core.BackupState.READY) {
                    announcedReady = true
                    NotdotState.setStatus("Listening hands-free · Say Hey Ava")
                }
                // Level only (never audio) for diagnostics, once a second.
                var sum = 0.0
                for (i in 0 until read) sum += packet[i] * packet[i]
                val rms = kotlin.math.sqrt(sum / read).toFloat()
                if (announcedReady) NotdotState.wakeInput(rms, wake?.backupBusy() == true)
                meterSum += rms; meterCount++; meterPeak = maxOf(meterPeak, rms)
                if (meterCount * read >= rate) {
                    val effects = NotdotState.handle(SessionEvent.Tick)
                    if (effects.endSurface) {
                        android.os.Handler(android.os.Looper.getMainLooper()).post { stop(SessionEvent.SystemStopped) }
                        break
                    }
                    // Backup decoder state and counters (no audio or text).
                    val backup = wake?.let { w ->
                        val s = w.backupStats()
                        "${w.backupState()} decodes=${s.decodes} wakes=${s.wakes} failures=${s.failures} decode_ms_total=${s.decodeMsTotal} decode_ms_max=${s.decodeMsMax}"
                    } ?: "none"
                    android.util.Log.i("notdot.voice", "capture rate=$rate mean_rms=${meterSum / meterCount} peak_rms=$meterPeak backup=$backup")
                    meterSum = 0f; meterCount = 0; meterPeak = 0f
                }
                if (wake?.accept(packet.copyOf(read).toList(), rate.toUInt()) == true) {
                    android.util.Log.i("notdot.voice", "wake detected")
                    NotdotState.wakeHeard()
                }
            }
        }
    }

    private fun stop(event: SessionEvent) {
        capturing = false
        recorder?.run { stop(); release() }
        recorder = null
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
