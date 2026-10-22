package org.zevaryx.rettui

import android.content.Context
import java.io.File
import java.util.concurrent.TimeUnit

/** rettui as the app has it: where its program and data are, and its commands. */
object Rettui {
    /** rettui's data directory: identity, settings, messages. */
    fun dataDir(context: Context) = File(context.filesDir, "rettui")

    /** The program, packed in the APK as a native library. */
    fun program(context: Context) = File(context.applicationInfo.nativeLibraryDir, "librettui.so")

    /** rettui run with the app's data directory and Reticulum config, then `args`. */
    fun builder(context: Context, vararg args: String): ProcessBuilder {
        val data = dataDir(context).apply { mkdirs() }
        val builder = ProcessBuilder(
            listOf(program(context).path, "--data-dir", data.path, "--rns-config", File(data, "reticulum").path) + args,
        )
        builder.environment()["HOME"] = context.filesDir.path
        builder.environment()["TMPDIR"] = context.cacheDir.path
        builder.redirectErrorStream(true)
        return builder
    }

    /** What one of rettui's commands said, and whether it worked. */
    data class Said(val ok: Boolean, val text: String)

    /** Run one of rettui's commands (`identity export`, `backup`…) to its end. */
    fun command(context: Context, vararg args: String): Said = try {
        val process = builder(context, *args).start()
        val text = process.inputStream.bufferedReader().readText()
        if (process.waitFor(5, TimeUnit.MINUTES)) {
            Said(process.exitValue() == 0, text.trim())
        } else {
            process.destroyForcibly()
            Said(false, "rettui took too long")
        }
    } catch (e: Exception) {
        Said(false, "Couldn't run rettui: $e")
    }
}
