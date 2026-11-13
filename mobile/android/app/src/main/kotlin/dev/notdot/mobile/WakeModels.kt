package dev.notdot.mobile

import android.content.Context
import uniffi.notdot_mobile_core.WakeEngine
import java.io.File

/**
 * The desktop's wake models, packaged as assets (`wake/`) and copied to
 * private storage once, because sherpa-onnx reads files by path.
 */
object WakeModels {
    @Volatile private var cached: WakeEngine? = null

    fun engine(context: Context, name: String = "Ava"): WakeEngine? = cached ?: runCatching {
        val dir = File(context.noBackupFilesDir, "wake")
        copy(context, "wake", dir)
        WakeEngine(dir.absolutePath, name, false)
    }.getOrNull().also { cached = it }

    private fun copy(context: Context, asset: String, target: File) {
        val children = context.assets.list(asset).orEmpty()
        if (children.isEmpty()) {
            if (!target.exists()) context.assets.open(asset).use { input -> target.outputStream().use(input::copyTo) }
            return
        }
        target.mkdirs()
        for (child in children) copy(context, "$asset/$child", File(target, child))
    }
}
