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
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import android.system.Os
import android.util.Log
import org.json.JSONObject
import java.io.File
import java.net.HttpURLConnection
import java.net.InetSocketAddress
import java.net.Socket
import java.net.URL

/**
 * Runs rettui (loaded from the APK as librettui.so) with its web UI on
 * this phone only, and keeps it running while the app is closed: a
 * foreground service, with its notification. It also listens to the web
 * UI's live events and shows rettui's notifications (messages, mentions)
 * as Android's, since a WebView can't.
 *
 * It has a process of its own (`:rettui`): rettui keeps some state for
 * as long as its process lives, so each start is in a new one, and the
 * process ends with it.
 */
class RettuiService : Service() {
    companion object {
        private const val TAG = "rettui"
        const val ACTION_STOP = "org.zevaryx.rettui.STOP"
        private const val CHANNEL_SERVICE = "service"
        private const val CHANNEL_MESSAGES = "messages"
        private const val ID_SERVICE = 1

        /** What rettui printed last run, shown if it stops. */
        fun logFile(context: Context) = File(context.filesDir, "rettui-output.txt")

        /** The web UI's login token, once rettui has made it. */
        fun token(context: Context): String? =
            File(Rettui.dataDir(context), "web_token").takeIf { it.isFile }?.readText()?.trim()?.takeIf { it.length >= 32 }

        /** Whether the web UI answers. */
        fun listening(context: Context): Boolean = try {
            Socket().use { it.connect(InetSocketAddress("127.0.0.1", Rettui.port(context)), 300) }
            true
        } catch (e: Exception) {
            false
        }

        fun start(context: Context) {
            context.startForegroundService(Intent(context, RettuiService::class.java))
        }

        /**
         * Stop rettui and wait until it has saved everything and its process
         * has ended (as restoring a backup needs). Whether it has.
         */
        fun stopAndWait(context: Context, seconds: Long = 25): Boolean {
            context.stopService(Intent(context, RettuiService::class.java))
            val until = System.currentTimeMillis() + seconds * 1000
            while (Rettui.running(context)) {
                if (System.currentTimeMillis() > until) return false
                Thread.sleep(200)
            }
            return true
        }
    }

    @Volatile
    private var running = false
    private var runner: Thread? = null
    private var listener: Thread? = null
    private var wakeLock: PowerManager.WakeLock? = null
    private var multicastLock: WifiManager.MulticastLock? = null
    private val main = Handler(Looper.getMainLooper())

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

        val log = logFile(this).apply { writeText("") }
        Os.setenv("RUST_BACKTRACE", "1", true)
        val args = Rettui.args(this, "--web", "127.0.0.1:${Rettui.choosePort(this)}")
        runner = Thread({
            val code = try {
                Native.run(args, log.path)
            } catch (e: Throwable) {
                log.appendText("Couldn't start rettui: $e\n")
                -1
            }
            Log.i(TAG, "rettui stopped ($code)")
            // Stopped by itself (it failed, or its process is ending): the
            // service goes too.
            main.post { stopSelf() }
        }, "rettui").apply { start() }
        listener = Thread(::listen, "rettui-events").apply { isDaemon = true; start() }
    }

    override fun onDestroy() {
        running = false
        listener?.interrupt()
        listener = null
        wakeLock?.takeIf { it.isHeld }?.release()
        multicastLock?.takeIf { it.isHeld }?.release()
        val stopping = runner
        runner = null
        // rettui saves everything and stops; then its process ends, to
        // start clean next time.
        Native.stop()
        Thread({
            stopping?.join(20_000)
            android.os.Process.killProcess(android.os.Process.myPid())
        }, "rettui-stop").start()
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
                if (token == null || !listening(this)) {
                    Thread.sleep(1000)
                    continue
                }
                val since = prefs.getString("since", null)
                val url = URL("http://127.0.0.1:${Rettui.port(this)}/api/events" + (since?.let { "?since=$it" } ?: ""))
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
                "notify" -> if (!Rettui.showing(this)) {
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
