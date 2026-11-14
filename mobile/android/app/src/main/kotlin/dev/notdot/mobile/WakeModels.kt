package dev.notdot.mobile

import android.content.Context
import uniffi.notdot_mobile_core.WakeEngine
import java.io.File

/**
 * The desktop's wake models, packaged as assets (`wake/`) and copied to
 * private storage per app update, because the local detector reads files by
 * path. Each file is written to a temporary name and renamed, so an
 * interrupted copy is redone rather than leaving a truncated model.
 */
object WakeModels {
    private data class Cached(val folder: File, val name: String, val engine: WakeEngine)
    @Volatile private var cached: Cached? = null

    /** Synchronized: two first uses must not copy the same files at once. */
    @Synchronized
    fun engine(context: Context, name: String = "Ava"): WakeEngine? = runCatching {
        val folder = directory(context)
        cached?.takeIf { it.folder == folder && it.name == name }?.engine ?: run {
            val engine = WakeEngine(folder.absolutePath, name, false)
            cached = Cached(folder, name, engine)
            engine
        }
    }.getOrNull()

    @Synchronized
    fun directory(context: Context): File {
        // Package update time changes even for debug builds with the same
        // version code. Never silently reuse weights from an older bundle.
        val version = context.packageManager.getPackageInfo(context.packageName, 0).lastUpdateTime
        return install(context, File(context.noBackupFilesDir, "wake"), version)
    }

    @Synchronized
    internal fun install(context: Context, root: File, version: Long): File {
        require(version >= 0)
        val dir = File(root, version.toString())
        copy(context, "wake", dir)
        // This folder contains generated model copies only, never user data.
        // Remove obsolete version folders and the pre-versioned flat layout
        // only after this bundle has copied fully.
        root.listFiles().orEmpty().filter { it != dir }
            .forEach { it.deleteRecursively() }
        return dir
    }

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
