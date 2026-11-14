package org.zevaryx.rettui

import android.app.Notification
import android.app.Person
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.RemoteInput
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.content.pm.ShortcutInfo
import android.content.pm.ShortcutManager
import android.graphics.drawable.Icon
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.Uri
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
        const val CHANNEL_MESSAGES = "messages"
        /** All of rettui's notifications about messages, rooms and games, for Android to show as one stack. */
        const val GROUP = "rettui"
        private const val ID_SUMMARY = 2

    /**
     * One notification to stand for the others when there are two or more:
     * "3 notifications", with what each says. Android shows them as a stack
     * under it, and takes it away with the last of them.
     */
    fun refreshSummary(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java)
        val children = try {
            manager.activeNotifications.filter {
                it.tag != null && it.notification.group == GROUP && it.notification.flags and Notification.FLAG_GROUP_SUMMARY == 0
            }
        } catch (e: Exception) {
            return
        }
        // Android takes a summary's others with it when it goes: only when none are left.
        // (One alone is shown without it.)
        if (children.size < 2) {
            if (children.isEmpty()) manager.cancel(ID_SUMMARY)
            return
        }
        val inbox = Notification.InboxStyle().setSummaryText(context.getString(R.string.notifications_waiting, children.size))
        for (child in children.take(6)) {
            val extras = child.notification.extras
            inbox.addLine("${extras.getCharSequence(Notification.EXTRA_TITLE) ?: ""}: ${extras.getCharSequence(Notification.EXTRA_TEXT) ?: ""}")
        }
        val open = PendingIntent.getActivity(context, 0, Intent(context, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
        manager.notify(
            ID_SUMMARY,
            Notification.Builder(context, CHANNEL_MESSAGES)
                .setSmallIcon(R.drawable.ic_notification)
                .setColor(context.getColor(R.color.accent))
                .setContentTitle(context.getString(R.string.app_name))
                .setContentText(context.getString(R.string.notifications_waiting, children.size))
                .setStyle(inbox)
                .setGroup(GROUP)
                .setGroupSummary(true)
                // Quiet itself: the ones under it make the sound.
                .setGroupAlertBehavior(Notification.GROUP_ALERT_CHILDREN)
                .setContentIntent(open)
                .setAutoCancel(true)
                .build(),
        )
    }
        private const val CHANNEL_ROOMS = "rooms"
        private const val CHANNEL_GAMES = "games"
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
    private var networkWatch: ConnectivityManager.NetworkCallback? = null
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
        // Apart from messages, so each can be turned off (or made quiet) on its own.
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL_ROOMS, getString(R.string.channel_rooms), NotificationManager.IMPORTANCE_HIGH),
        )
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL_GAMES, getString(R.string.channel_games), NotificationManager.IMPORTANCE_DEFAULT),
        )
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            // Stopped on purpose: the app says so, rather than wait for it.
            Rettui.setStoppedByYou(this, true)
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
        Rettui.setStoppedByYou(this, false)
        // Held only while rettui asks (see applyLocks): both cost battery.
        wakeLock = getSystemService(PowerManager::class.java)
            .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "rettui:reticulum").apply { setReferenceCounted(false) }
        multicastLock = getSystemService(WifiManager::class.java)
            .createMulticastLock("rettui").apply { setReferenceCounted(false) }

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
            // It ended cleanly while the service still wanted it (the web
            // UI's Stop rettui): as with Stop, the app says so.
            if (code == 0 && running) Rettui.setStoppedByYou(this, true)
            // Stopped by itself (it failed, or its process is ending): the
            // service goes too.
            main.post { stopSelf() }
        }, "rettui").apply { start() }
        listener = Thread(::listen, "rettui-events").apply { isDaemon = true; start() }
        watchNetwork()
    }

    /**
     * When the phone's own network changes to one that isn't mobile data
     * (Wi-Fi is back), rettui's connections still on mobile data reconnect
     * over it. One made while Wi-Fi was away for a moment otherwise stays
     * on mobile data, which the phone keeps up, and keeps its radio awake.
     */
    private fun watchNetwork() {
        val watch = object : ConnectivityManager.NetworkCallback() {
            private var current: Network? = null

            override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) {
                // Called again for the same network as its signal changes.
                if (network == current) return
                current = network
                if (!running) return
                val moved = try {
                    Native.networkChanged(capabilities.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR))
                } catch (e: Throwable) {
                    0
                }
                if (moved > 0) Log.i(TAG, "Reconnecting $moved connections that were on mobile data")
            }

            override fun onLost(network: Network) {
                if (network == current) current = null
            }
        }
        runCatching { getSystemService(ConnectivityManager::class.java).registerDefaultNetworkCallback(watch) }
            .onSuccess { networkWatch = watch }
    }

    /**
     * Hold what rettui asks for (its `Native.wants()`), and nothing else: a
     * wake lock with Keep the phone awake on (otherwise the phone sleeps,
     * and incoming traffic still wakes it), and Wi-Fi's multicast lock while
     * an AutoInterface is on (it hears its peers by multicast, which Wi-Fi
     * drops unless asked).
     */
    @Synchronized
    private fun applyLocks() {
        if (!running) return
        val wants = try {
            Native.wants()
        } catch (e: Throwable) {
            0
        }
        wakeLock?.let { if (wants and 1 != 0) { if (!it.isHeld) it.acquire() } else if (it.isHeld) it.release() }
        multicastLock?.let { if (wants and 2 != 0) { if (!it.isHeld) it.acquire() } else if (it.isHeld) it.release() }
    }

    override fun onDestroy() {
        listener?.interrupt()
        listener = null
        networkWatch?.let { runCatching { getSystemService(ConnectivityManager::class.java).unregisterNetworkCallback(it) } }
        networkWatch = null
        // After this, applyLocks takes nothing again.
        synchronized(this) {
            running = false
            wakeLock?.takeIf { it.isHeld }?.release()
            multicastLock?.takeIf { it.isHeld }?.release()
        }
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
            .setColor(getColor(R.color.accent))
            .setContentTitle(getString(R.string.running))
            .setContentText(getString(R.string.running_detail))
            .setContentIntent(open)
            .setOngoing(true)
            .addAction(Notification.Action.Builder(null, getString(R.string.stop), stop).build())
            .build()
    }

    // ---- rettui's notifications ---------------------------------------------

    /** Reply or Mark read, for the notification `tag` about `target`. */
    private fun action(name: String, label: Int, tag: String, target: String, reply: Boolean): Notification.Action {
        val intent = Intent(this, NotificationActions::class.java)
            .setAction(name)
            .putExtra(NotificationActions.EXTRA_TAG, tag)
            .putExtra(NotificationActions.EXTRA_TARGET, target)
        // Android puts the reply in the intent, so that one stays mutable.
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or if (reply && Build.VERSION.SDK_INT >= 31) PendingIntent.FLAG_MUTABLE else PendingIntent.FLAG_IMMUTABLE
        val pending = PendingIntent.getBroadcast(this, (name + tag).hashCode(), intent, flags)
        val builder = Notification.Action.Builder(null, getString(label), pending)
        if (reply) builder.addRemoteInput(RemoteInput.Builder(NotificationActions.REPLY).setLabel(getString(R.string.reply)).build())
        // What they are, for a watch or a car to show them as such.
        if (Build.VERSION.SDK_INT >= 28) {
            builder.setSemanticAction(if (reply) Notification.Action.SEMANTIC_ACTION_REPLY else Notification.Action.SEMANTIC_ACTION_MARK_AS_READ)
        }
        return builder.build()
    }

    /** The web UI's live events, for as long as the service runs. */
    private fun listen() {
        val prefs = getSharedPreferences("events", MODE_PRIVATE)
        while (running && !Thread.currentThread().isInterrupted) {
            applyLocks()
            try {
                val token = token(this)
                if (token == null || !listening(this)) {
                    Thread.sleep(1000)
                    continue
                }
                val since = prefs.getString("since", null)
                // Notifications only: what changed (every announce, on a busy
                // network) is the web UI's business, and would wake this.
                val url = URL("http://127.0.0.1:${Rettui.port(this)}/api/events?notices=1" + (since?.let { "&since=$it" } ?: ""))
                val connection = url.openConnection() as HttpURLConnection
                connection.setRequestProperty("Authorization", "Bearer $token")
                connection.setRequestProperty("Accept", "text/event-stream")
                connection.readTimeout = 0
                try {
                    var event = ""
                    var id: String? = null
                    val data = StringBuilder()
                    connection.inputStream.bufferedReader().forEachLine { line ->
                        // A line at least every 15 s (the stream's keep-alive):
                        // a changed setting takes effect by then.
                        applyLocks()
                        refreshSummary(this)
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

    /** One line of a conversation's notification: who said it, what, and when. */
    private data class Line(val who: String, val text: String, val at: Long)

    /** What each conversation's or room's notification shows (by its tag), newest last. */
    private val said = mutableMapOf<String, MutableList<Line>>()

    /**
     * A conversation's or room's notification as a chat: each message under
     * the one before, rather than the newest alone. Its lines start over
     * once it has gone (read, or swiped aside). Nothing for other
     * notifications (a game's move, a summary).
     */
    private fun chat(manager: NotificationManager, tag: String, target: JSONObject?, title: String, body: String): Notification.Style? {
        val kind = target?.optString("kind")
        if (kind != "conversation" && kind != "room") return null
        val room = target.optString("room")
        // In a room, the title says who and where ("Ann in #general"); the
        // room is the chat's title instead.
        val who = if (kind == "room" && room.isNotEmpty()) title.removeSuffix(" in #$room") else title
        val lines = synchronized(said) {
            val showing = manager.activeNotifications.any { it.tag == tag }
            val lines = if (showing) said.getOrPut(tag) { mutableListOf() } else mutableListOf<Line>().also { said[tag] = it }
            lines.add(Line(who, body, System.currentTimeMillis()))
            // As many as a notification shows.
            while (lines.size > 8) lines.removeAt(0)
            lines.toList()
        }
        if (Build.VERSION.SDK_INT >= 28) return personStyle(kind, room, title, target.optString("key"), lines)
        // By name: Android's Person is only from Android 9 on.
        @Suppress("DEPRECATION")
        val style = Notification.MessagingStyle(getString(R.string.you))
        if (kind == "room" && room.isNotEmpty() && !title.endsWith("(whisper)")) {
            style.setConversationTitle("#$room")
            if (Build.VERSION.SDK_INT >= 28) style.setGroupConversation(true)
        }
        @Suppress("DEPRECATION")
        for (line in lines) style.addMessage(Notification.MessagingStyle.Message(line.text, line.at, line.who))
        return style
    }

    /**
     * The same as a chat, with a Person for each side: Android shows the
     * contact's picture by their messages, and (with a shortcut, see
     * [publishShortcut]) treats it as a conversation.
     */
    @android.annotation.TargetApi(28)
    private fun personStyle(kind: String, room: String, title: String, key: String, lines: List<Line>): Notification.Style {
        val style = Notification.MessagingStyle(Person.Builder().setName(getString(R.string.you)).build())
        if (kind == "room" && room.isNotEmpty() && !title.endsWith("(whisper)")) {
            style.setConversationTitle("#$room")
            style.setGroupConversation(true)
        }
        val others = mutableMapOf<String, Person>()
        for (line in lines) {
            val person = others.getOrPut(line.who) { person(if (kind == "conversation") key else "", line.who) }
            style.addMessage(Notification.MessagingStyle.Message(line.text, line.at, person))
        }
        return style
    }

    /** Someone, with their face if `key` is an address rettui has one for. */
    @android.annotation.TargetApi(28)
    private fun person(key: String, name: String): Person {
        val builder = Person.Builder().setName(name)
        if (key.isNotEmpty()) {
            builder.setKey(key)
            Avatars.face(this, key)?.let { builder.setIcon(Icon.createWithBitmap(it)) }
        }
        return builder.build()
    }

    /**
     * What makes Android 11 and later treat a notification as a conversation
     * (in their own section, with their own settings, and shown in sharing):
     * a long-lived shortcut for the person, which the notification names.
     * Opening it opens the conversation, as tapping the notification does.
     */
    @android.annotation.TargetApi(30)
    private fun publishShortcut(key: String, name: String, target: String): String {
        val id = "conversation:$key"
        runCatching {
            val open = Intent(this, MainActivity::class.java).setAction(Intent.ACTION_VIEW).putExtra(MainActivity.EXTRA_TARGET, target)
            val info = ShortcutInfo.Builder(this, id)
                .setShortLabel(name.take(40))
                .setLongLived(true)
                .setPerson(person(key, name))
                .setIntent(open)
                .apply { Avatars.face(this@RettuiService, key)?.let { setIcon(Icon.createWithBitmap(it)) } }
                .build()
            getSystemService(ShortcutManager::class.java).pushDynamicShortcut(info)
        }.onFailure { Log.i(TAG, "Couldn't make a shortcut: $it") }
        return id
    }

    /**
     * A conversation as a bubble, if the phone offers them (Android 11 and
     * later, and turned on for rettui or this person): a button on the
     * notification floats the conversation over other apps.
     */
    @android.annotation.TargetApi(30)
    private fun bubble(key: String, tag: String, target: String): Notification.BubbleMetadata? {
        val face = Avatars.face(this, key) ?: return null
        val open = PendingIntent.getActivity(
            this,
            ("bubble:" + tag).hashCode(),
            Intent(this, BubbleActivity::class.java).setAction(Intent.ACTION_VIEW).putExtra(MainActivity.EXTRA_TARGET, target),
            // A bubble's intent has to be mutable: Android fills in its size.
            PendingIntent.FLAG_MUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return Notification.BubbleMetadata.Builder(open, Icon.createWithAdaptiveBitmap(face)).setDesiredHeight(640).build()
    }

    /**
     * A conversation read in its own bubble keeps the bubble: cancelling the
     * notification would close it. The notification goes out of the shade
     * instead (Android's "suppress"), and comes back with the next message.
     * Whether it did (else the notification should go).
     */
    private fun keepBubble(manager: NotificationManager, tag: String): Boolean {
        if (Build.VERSION.SDK_INT < 30 || !Rettui.bubbleShowing(this)) return false
        return try {
            val shown = manager.activeNotifications.firstOrNull { it.tag == tag } ?: return false
            val bubble = shown.notification.bubbleMetadata ?: return false
            val intent = bubble.intent ?: return false
            val icon = bubble.icon ?: return false
            val updated = Notification.Builder.recoverBuilder(this, shown.notification)
                .setBubbleMetadata(Notification.BubbleMetadata.Builder(intent, icon).setDesiredHeight(640).setSuppressNotification(true).build())
                .setOnlyAlertOnce(true)
                .build()
            manager.notify(tag, 0, updated)
            true
        } catch (e: Exception) {
            false
        }
    }

    /**
     * Accept and Decline on a notification about a challenge: only while it
     * waits for an answer (the game itself says which buttons it has).
     */
    private fun gameActions(builder: Notification.Builder, tag: String, target: String, json: JSONObject?) {
        val app = Uri.encode(json?.optString("app").orEmpty())
        val session = Uri.encode(json?.optString("session").orEmpty())
        if (app.isEmpty() || session.isEmpty()) return
        val game = runCatching { JSONObject(Rettui.api(this, "games/$app/$session").text) }.getOrNull() ?: return
        val buttons = game.optJSONArray("actions") ?: return
        val offered = (0 until buttons.length()).map { buttons.optString(it) }
        if (game.optString("status") != "pending" || "accept" !in offered) return
        builder.addAction(action(NotificationActions.ACTION_ACCEPT, R.string.accept, tag, target, reply = false))
        builder.addAction(action(NotificationActions.ACTION_DECLINE, R.string.decline, tag, target, reply = false))
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
                    val title = json.optString("title").take(200)
                    val body = json.optString("body").take(1000)
                    val channel = when (json.optJSONObject("target")?.optString("kind")) {
                        "room" -> CHANNEL_ROOMS
                        "game" -> CHANNEL_GAMES
                        else -> CHANNEL_MESSAGES
                    }
                    val builder = Notification.Builder(this, channel)
                        .setSmallIcon(R.drawable.ic_notification)
                        .setColor(getColor(R.color.accent))
                        .setContentTitle(title)
                        .setContentText(body)
                        .setStyle(chat(manager, tag, json.optJSONObject("target"), title, body) ?: Notification.BigTextStyle().bigText(body))
                        .setContentIntent(open)
                        .setAutoCancel(true)
                        // A message: Do Not Disturb's "messages" exceptions apply.
                        .setCategory(Notification.CATEGORY_MESSAGE)
                        .setGroup(GROUP)
                    val kind = json.optJSONObject("target")?.optString("kind")
                    if (kind == "conversation" && Build.VERSION.SDK_INT >= 30) {
                        val key = json.optJSONObject("target")?.optString("key").orEmpty()
                        if (key.isNotEmpty()) {
                            builder.setShortcutId(publishShortcut(key, title, target))
                            // Not auto-cancelled: opening the bubble counts as a tap, which would
                            // take it away. Reading the conversation takes it away instead.
                            bubble(key, tag, target)?.let { builder.setBubbleMetadata(it).setAutoCancel(false) }
                        }
                    }
                    if (kind == "game") gameActions(builder, tag, target, json.optJSONObject("target"))
                    // A conversation or a room can be answered, or marked
                    // read, from here.
                    if (kind in setOf("conversation", "room")) {
                        builder.addAction(action(NotificationActions.ACTION_REPLY, R.string.reply, tag, target, reply = true))
                        builder.addAction(action(NotificationActions.ACTION_READ, R.string.mark_read, tag, target, reply = false))
                    }
                    manager.notify(tag, 0, builder.build())
                    // Once Android has it (posting takes a moment).
                    main.postDelayed({ refreshSummary(this) }, 500)
                }
                // Read, here or elsewhere: its notification goes.
                "read" -> {
                    if (!keepBubble(manager, tag)) manager.cancel(tag, 0)
                    synchronized(said) { said.remove(tag) }
                    main.postDelayed({ refreshSummary(this) }, 500)
                }
            }
        } catch (e: Exception) {
            Log.i(TAG, "Couldn't read a $event event: $e")
        }
    }
}
