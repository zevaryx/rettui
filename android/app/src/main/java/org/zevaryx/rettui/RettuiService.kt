package org.zevaryx.rettui

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.util.Log
import org.json.JSONObject
import java.io.File
import java.net.HttpURLConnection
import java.net.InetSocketAddress
import java.net.Socket
import java.net.URL
import java.util.concurrent.TimeUnit

/**
 * Runs rettui (the program packed in the APK as librettui.so) with its web
 * UI on this phone only, and keeps it running while the app is closed: a
 * foreground service, with its notification. It also listens to the web
 * UI's live events and shows rettui's notifications (messages, mentions)
 * as Android's, since a WebView can't.
 */
class RettuiService : Service() {
    companion object {
        private const val TAG = "rettui"
        const val PORT = 8740
        const val ACTION_STOP = "org.zevaryx.rettui.STOP"
        private const val CHANNEL_SERVICE = "service"
        private const val CHANNEL_MESSAGES = "messages"
        private const val ID_SERVICE = 1

        /** Whether rettui is running (or starting), for the activity. */
        @Volatile
        var running = false
            private set

        /** rettui's data directory: identity, settings, messages. */
        fun dataDir(context: Context) = File(context.filesDir, "rettui")

        /** What rettui printed last run, shown if it stops. */
        fun logFile(context: Context) = File(context.filesDir, "rettui-output.txt")

        /** The web UI's login token, once rettui has made it. */
        fun token(context: Context): String? =
            File(dataDir(context), "web_token").takeIf { it.isFile }?.readText()?.trim()?.takeIf { it.length >= 32 }

        /** Whether the web UI answers. */
        fun listening(): Boolean = try {
            Socket().use { it.connect(InetSocketAddress("127.0.0.1", PORT), 300) }
            true
        } catch (e: Exception) {
            false
        }

        fun start(context: Context) {
            context.startForegroundService(Intent(context, RettuiService::class.java))
        }
    }

    private var process: Process? = null
    private var listener: Thread? = null
    private var wakeLock: PowerManager.WakeLock? = null
    private var multicastLock: WifiManager.MulticastLock? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL_SERVICE, getString(R.string.channel_service), NotificationManager.IMPORTANCE_MIN),
        )
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL_MESSAGES, getString(R.string.channel_messages), NotificationManager.IMPORTANCE_HIGH),
        )
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            stopSelf()
            return START_NOT_STICKY
        }
        if (Build.VERSION.SDK_INT >= 34) {
            startForeground(ID_SERVICE, serviceNotification(), ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        } else {
            startForeground(ID_SERVICE, serviceNotification())
        }
        if (!running) launch()
        return START_STICKY
    }

    private fun launch() {
        running = true
        // Reticulum keeps links alive in the background, and AutoInterface
        // hears its peers by multicast, which Wi-Fi drops unless asked.
        wakeLock = getSystemService(PowerManager::class.java)
            .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "rettui:reticulum").apply { acquire() }
        multicastLock = getSystemService(WifiManager::class.java)
            .createMulticastLock("rettui").apply { setReferenceCounted(false); acquire() }

        // One left over from before (the app was stopped, its program
        // wasn't): use it.
        if (listening()) {
            Log.i(TAG, "rettui is running already")
        } else {
            val data = dataDir(this).apply { mkdirs() }
            val exe = File(applicationInfo.nativeLibraryDir, "librettui.so")
            val builder = ProcessBuilder(
                exe.path,
                "--data-dir", data.path,
                "--rns-config", File(data, "reticulum").path,
                "--web", "127.0.0.1:$PORT",
            )
            builder.environment()["HOME"] = filesDir.path
            builder.environment()["TMPDIR"] = cacheDir.path
            builder.environment()["RUST_BACKTRACE"] = "1"
            builder.redirectErrorStream(true)
            builder.redirectOutput(logFile(this))
            val started = try {
                builder.start()
            } catch (e: Exception) {
                logFile(this).writeText("Couldn't start rettui: $e\n")
                stopSelf()
                return
            }
            process = started
            Thread({
                val code = started.waitFor()
                Log.i(TAG, "rettui stopped ($code)")
                if (process === started) {
                    process = null
                    stopSelf()
                }
            }, "rettui-wait").start()
        }
        listener = Thread(::listen, "rettui-events").apply { isDaemon = true; start() }
    }

    override fun onDestroy() {
        running = false
        listener?.interrupt()
        listener = null
        process?.let { stopping ->
            process = null
            // SIGTERM: rettui saves everything, then exits.
            stopping.destroy()
            Thread({
                if (!stopping.waitFor(10, TimeUnit.SECONDS)) stopping.destroyForcibly()
            }, "rettui-stop").start()
        }
        wakeLock?.takeIf { it.isHeld }?.release()
        multicastLock?.takeIf { it.isHeld }?.release()
        super.onDestroy()
    }

    private fun serviceNotification(): Notification {
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE,
        )
        val stop = PendingIntent.getService(
            this, 1, Intent(this, RettuiService::class.java).setAction(ACTION_STOP), PendingIntent.FLAG_IMMUTABLE,
        )
        return Notification.Builder(this, CHANNEL_SERVICE)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(getString(R.string.running))
            .setContentText(getString(R.string.running_detail))
            .setContentIntent(open)
            .setOngoing(true)
            .addAction(Notification.Action.Builder(null, getString(R.string.stop), stop).build())
            .build()
    }

    // ---- rettui's notifications ---------------------------------------------

    /** The web UI's live events, for as long as the service runs. */
    private fun listen() {
        val prefs = getSharedPreferences("events", MODE_PRIVATE)
        while (running && !Thread.currentThread().isInterrupted) {
            try {
                val token = token(this)
                if (token == null || !listening()) {
                    Thread.sleep(1000)
                    continue
                }
                val since = prefs.getString("since", null)
                val url = URL("http://127.0.0.1:$PORT/api/events" + (since?.let { "?since=$it" } ?: ""))
                val connection = url.openConnection() as HttpURLConnection
                connection.setRequestProperty("Authorization", "Bearer $token")
                connection.setRequestProperty("Accept", "text/event-stream")
                connection.readTimeout = 0
                try {
                    var event = ""
                    var id: String? = null
                    val data = StringBuilder()
                    connection.inputStream.bufferedReader().forEachLine { line ->
                        when {
                            line.isEmpty() -> {
                                if (event.isNotEmpty() && data.isNotEmpty()) {
                                    handle(event, data.toString())
                                    if (event == "notify" && id != null) prefs.edit().putString("since", id).apply()
                                }
                                event = ""
                                id = null
                                data.setLength(0)
                            }
                            line.startsWith("event:") -> event = line.substring(6).trim()
                            line.startsWith("id:") -> id = line.substring(3).trim()
                            line.startsWith("data:") -> {
                                if (data.isNotEmpty()) data.append('\n')
                                data.append(line.substring(5).removePrefix(" "))
                            }
                        }
                    }
                } finally {
                    connection.disconnect()
                }
            } catch (e: InterruptedException) {
                return
            } catch (e: Exception) {
                Log.i(TAG, "Live events: $e")
            }
            try {
                Thread.sleep(2000)
            } catch (e: InterruptedException) {
                return
            }
        }
    }

    private fun handle(event: String, data: String) {
        val manager = getSystemService(NotificationManager::class.java)
        try {
            val json = JSONObject(data)
            val tag = json.optString("tag")
            when (event) {
                // The web UI on screen shows its own.
                "notify" -> if (!MainActivity.visible) {
                    val target = json.optJSONObject("target")?.toString() ?: "{}"
                    val open = PendingIntent.getActivity(
                        this,
                        tag.hashCode(),
                        Intent(this, MainActivity::class.java).putExtra(MainActivity.EXTRA_TARGET, target),
                        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                    )
                    val notification = Notification.Builder(this, CHANNEL_MESSAGES)
                        .setSmallIcon(R.drawable.ic_notification)
                        .setContentTitle(json.optString("title").take(200))
                        .setContentText(json.optString("body").take(1000))
                        .setStyle(Notification.BigTextStyle().bigText(json.optString("body").take(1000)))
                        .setContentIntent(open)
                        .setAutoCancel(true)
                        .build()
                    manager.notify(tag, 0, notification)
                }
                // Read, here or elsewhere: its notification goes.
                "read" -> manager.cancel(tag, 0)
            }
        } catch (e: Exception) {
            Log.i(TAG, "Couldn't read a $event event: $e")
        }
    }
}
