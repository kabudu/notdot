package dev.notdot.mobile

import android.content.Context
import uniffi.notdot_mobile_core.WakeEngine
import java.io.File

/**
 * The desktop's wake models, packaged as assets (`wake/`) and copied to
 * private storage once, because sherpa-onnx and whisper.cpp read files by
 * path. Each file is written to a temporary name and renamed, so an
 * interrupted copy is redone rather than leaving a truncated model.
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
            if (!target.exists()) {
                val partial = File(target.parentFile, "${target.name}.partial")
                context.assets.open(asset).use { input -> partial.outputStream().use(input::copyTo) }
                check(partial.renameTo(target)) { "could not install ${target.name}" }
            }
            return
        }
        target.mkdirs()
        for (child in children) copy(context, "$asset/$child", File(target, child))
    }
}
