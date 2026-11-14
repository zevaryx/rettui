package org.zevaryx.rettui

import android.app.ActivityManager
import android.content.Context
import android.system.Os
import org.json.JSONObject
import java.io.File
import java.net.HttpURLConnection
import java.net.InetAddress
import java.net.ServerSocket
import java.net.URL

/** rettui as the app has it: where its data is, its commands, and its process. */
object Rettui {
    /** The process rettui runs in (see the service in AndroidManifest.xml). */
    private const val PROCESS = ":rettui"

    /** rettui's data directory: identity, settings, messages. */
    fun dataDir(context: Context) = File(context.filesDir, "rettui")

    /** The web UI's usual port. */
    private const val PORT = 8740

    /** The port the web UI is on (kept in a file, for the app's other process). */
    fun port(context: Context): Int = runCatching { File(context.filesDir, "port").readText().trim().toInt() }.getOrDefault(PORT)

    /**
     * Pick the web UI's port before it starts: the one it had (so the page
     * keeps what this browser saved for it), or another app has that one:
     * any free one.
     */
    fun choosePort(context: Context): Int {
        val loopback = InetAddress.getByName("127.0.0.1")
        val free = { port: Int -> runCatching { ServerSocket(port, 1, loopback).use { it.localPort } }.getOrNull() }
        val port = free(port(context)) ?: free(0) ?: PORT
        File(context.filesDir, "port").writeText(port.toString())
        return port
    }

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
            Said(false, context.getString(R.string.command_failed, e.toString()))
        } finally {
            output.delete()
        }
    }

    /** The variable rettui takes a backup's passphrase from (it has no terminal here to ask). */
    private const val PASSPHRASE = "RETTUI_BACKUP_PASSPHRASE"

    /**
     * Run one of rettui's commands with `passphrase` as a backup's (if there
     * is one), set only while it runs: the process's environment, one
     * command at a time.
     */
    @Synchronized
    fun withPassphrase(context: Context, passphrase: String?, vararg args: String): Said {
        if (passphrase.isNullOrEmpty()) {
            Os.unsetenv(PASSPHRASE)
            return command(context, *args)
        }
        return try {
            Os.setenv(PASSPHRASE, passphrase, true)
            command(context, *args)
        } finally {
            Os.unsetenv(PASSPHRASE)
        }
    }

    /** The start of a backup sealed with a passphrase. */
    private val SEALED = "rettui sealed backup1\n".toByteArray()

    /** Whether `input` (a file, from its start) is a backup sealed with a passphrase. */
    fun sealed(input: java.io.InputStream): Boolean {
        val start = ByteArray(SEALED.size)
        var got = 0
        while (got < start.size) {
            val n = input.read(start, got, start.size - got)
            if (n < 0) return false
            got += n
        }
        return start.contentEquals(SEALED)
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

    /** A conversation is open in its bubble (see BubbleActivity); a file too, for the service. */
    fun setBubbleShowing(context: Context, showing: Boolean) {
        val flag = File(context.filesDir, "bubble")
        if (showing) flag.writeText("") else flag.delete()
    }

    fun bubbleShowing(context: Context) = File(context.filesDir, "bubble").exists()

    /** rettui was stopped with Stop (not by a restore, nor a crash). */
    fun setStoppedByYou(context: Context, stopped: Boolean) {
        val flag = File(context.filesDir, "stopped")
        if (stopped) flag.writeText("") else flag.delete()
    }

    fun stoppedByYou(context: Context) = File(context.filesDir, "stopped").exists()

    /**
     * Ask the web UI's API (`/api/<path>`), logged in with its token, as the
     * web UI itself does. What it answered, or why it couldn't.
     */
    fun api(context: Context, path: String, body: JSONObject? = null): Said = try {
        val token = RettuiService.token(context) ?: error(context.getString(R.string.not_running))
        val connection = URL("http://127.0.0.1:${port(context)}/api/$path").openConnection() as HttpURLConnection
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
