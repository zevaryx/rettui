package org.zevaryx.rettui

import android.app.ActivityManager
import android.content.Context
import android.system.Os
import org.json.JSONObject
import java.io.File
import java.net.HttpURLConnection
import java.net.URL

/** rettui as the app has it: where its data is, its commands, and its process. */
object Rettui {
    /** The process rettui runs in (see the service in AndroidManifest.xml). */
    private const val PROCESS = ":rettui"

    /** rettui's data directory: identity, settings, messages. */
    fun dataDir(context: Context) = File(context.filesDir, "rettui")

    /** rettui's arguments for the app's data directory and Reticulum config, then `args`. */
    fun args(context: Context, vararg args: String): Array<String> {
        val data = dataDir(context).apply { mkdirs() }
        Os.setenv("HOME", context.filesDir.path, true)
        Os.setenv("TMPDIR", context.cacheDir.path, true)
        return arrayOf("--data-dir", data.path, "--rns-config", File(data, "reticulum").path, *args)
    }

    /** What one of rettui's commands said, and whether it worked. */
    data class Said(val ok: Boolean, val text: String)

    /** Run one of rettui's commands (`identity export`, `backup`…) to its end. */
    fun command(context: Context, vararg args: String): Said {
        val output = File.createTempFile("said", ".txt", context.cacheDir)
        return try {
            val code = Native.run(args(context, *args), output.path)
            Said(code == 0, output.readText().trim())
        } catch (e: Throwable) {
            Said(false, "Couldn't run rettui: $e")
        } finally {
            output.delete()
        }
    }

    /** Whether rettui's process is there (starting, running or stopping). */
    fun running(context: Context): Boolean {
        val name = context.packageName + PROCESS
        return context.getSystemService(ActivityManager::class.java).runningAppProcesses.orEmpty().any { it.processName == name }
    }

    /**
     * The web UI is on screen (it shows notifications itself then). A file,
     * since the service is in a process of its own.
     */
    fun setShowing(context: Context, showing: Boolean) {
        val flag = File(context.filesDir, "showing")
        if (showing) flag.writeText("") else flag.delete()
    }

    fun showing(context: Context) = File(context.filesDir, "showing").exists()

    /**
     * Ask the web UI's API (`/api/<path>`), logged in with its token, as the
     * web UI itself does. What it answered, or why it couldn't.
     */
    fun api(context: Context, path: String, body: JSONObject? = null): Said = try {
        val token = RettuiService.token(context) ?: error("rettui isn't running")
        val connection = URL("http://127.0.0.1:${RettuiService.PORT}/api/$path").openConnection() as HttpURLConnection
        connection.setRequestProperty("Authorization", "Bearer $token")
        connection.connectTimeout = 3000
        connection.readTimeout = 15_000
        if (body != null) {
            connection.requestMethod = "POST"
            connection.doOutput = true
            connection.setRequestProperty("Content-Type", "application/json")
            connection.outputStream.use { it.write(body.toString().toByteArray()) }
        }
        val ok = connection.responseCode in 200..299
        val text = (if (ok) connection.inputStream else connection.errorStream)?.bufferedReader()?.readText().orEmpty()
        connection.disconnect()
        // Errors come as {"error": "..."}.
        Said(ok, if (ok) text else runCatching { JSONObject(text).getString("error") }.getOrDefault(text))
    } catch (e: Exception) {
        Said(false, e.message ?: e.toString())
    }
}
