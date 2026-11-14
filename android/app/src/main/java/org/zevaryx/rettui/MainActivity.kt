package org.zevaryx.rettui

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.app.AlertDialog
import android.content.ContentValues
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.provider.DocumentsContract
import android.provider.MediaStore
import android.provider.Settings
import android.util.Log
import android.view.Gravity
import android.window.OnBackInvokedCallback
import android.window.OnBackInvokedDispatcher
import android.view.ViewGroup
import android.webkit.CookieManager
import android.webkit.GeolocationPermissions
import android.webkit.PermissionRequest
import android.webkit.RenderProcessGoneDetail
import android.webkit.URLUtil
import android.webkit.ValueCallback
import android.webkit.WebChromeClient
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.Button
import android.widget.LinearLayout
import android.widget.ProgressBar
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast
import org.json.JSONObject
import java.io.File
import java.net.HttpURLConnection
import java.net.URL

/**
 * The web UI of the rettui this app runs (see [RettuiService]), in a
 * WebView: the same pages a browser shows, with the phone's microphone,
 * location and files where the pages ask for them.
 */
class MainActivity : Activity() {
    companion object {
        /**
         * Said in the WebView's user agent, so the web UI offers what only
         * the app has (its identity and backups screen).
         */
        const val AGENT = "rettui-android/1"

        /** The web UI's links to the app's own screens. */
        private val SCREENS = mapOf("rettui-app:backups" to BackupsActivity::class.java, "rettui-app:rnodes" to RNodesActivity::class.java, "rettui-app:downloads" to DownloadsActivity::class.java)

        /** What a notification tapped is about (JSON, as the web UI has it). */
        const val EXTRA_TARGET = "target"
        private const val ASK_START = 1
        private const val ASK_MICROPHONE = 2
        private const val ASK_LOCATION = 3
        private const val PICK_FILES = 4
        private const val SAVE_FILE = 5

        /** Links other apps may open with rettui (see AndroidManifest.xml). */
        private val SCHEMES = setOf("lxma", "lxmf", "nomadnetwork", "rrc")
    }

    private val main = Handler(Looper.getMainLooper())
    private lateinit var web: WebView
    private lateinit var waiting: LinearLayout
    /** The web UI's screen: the page, and what shows while rettui starts. */
    private lateinit var screen: LinearLayout
    private var loaded = false
    /** What the page is to do once it has loaded (a notification's, a share's). */
    private val pending = mutableListOf<String>()
    private var waitedSince = 0L
    private var pendingPermission: PermissionRequest? = null
    private var pendingLocation: Pair<String, GeolocationPermissions.Callback>? = null
    private var pendingFiles: ValueCallback<Array<Uri>>? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (Build.VERSION.SDK_INT >= 33) onBackInvokedDispatcher.registerOnBackInvokedCallback(OnBackInvokedDispatcher.PRIORITY_DEFAULT, back!!)
        handle(intent)
        waiting = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER
            addView(ProgressBar(context).apply {
                isIndeterminate = true
                indeterminateTintList = android.content.res.ColorStateList.valueOf(palette().accent)
            })
            addView(TextView(context).apply {
                text = getString(R.string.starting)
                gravity = Gravity.CENTER
                textSize = 16f
                setTextColor(palette().dim)
                setPadding(0, dp(16), 0, 0)
            })
        }
        web = WebView(this).apply { visibility = android.view.View.GONE }
        setUpWebView()
        showWebScreen()
        // Android 13 and later ask before an app shows notifications;
        // rettui starts either way.
        if (Build.VERSION.SDK_INT >= 33 && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), ASK_START)
        } else {
            begin()
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handle(intent)
    }

    /**
     * What the app was opened for: a notification tapped (what it's about),
     * text shared from another app, or a link to open. Only strings reach
     * the page, quoted; it parses and checks them itself.
     */
    private fun handle(intent: Intent) {
        intent.getStringExtra(EXTRA_TARGET)?.let { return page("try { openTarget(JSON.parse(${JSONObject.quote(it)})) } catch (e) { console.warn(e) }") }
        val streams = streams(intent)
        if (streams.isNotEmpty()) return shareFiles(intent.getStringExtra(Intent.EXTRA_TEXT).orEmpty(), streams)
        val shared = when (intent.action) {
            Intent.ACTION_SEND -> intent.getStringExtra(Intent.EXTRA_TEXT)
            Intent.ACTION_VIEW -> intent.data?.takeIf { it.scheme?.lowercase() in SCHEMES }?.toString()
            else -> null
        }
        shared?.takeIf { it.isNotBlank() }?.let { page("shareText(${JSONObject.quote(it.take(50_000))})") }
    }

    /** Files shared with the app (one, or several at once). */
    @Suppress("DEPRECATION")
    private fun streams(intent: Intent): List<Uri> = when (intent.action) {
        Intent.ACTION_SEND -> listOfNotNull(intent.getParcelableExtra(Intent.EXTRA_STREAM) as? Uri)
        Intent.ACTION_SEND_MULTIPLE -> intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM).orEmpty()
        else -> emptyList()
    }

    /** Copy them in (away from the screen), then hand them to the page. */
    private fun shareFiles(text: String, uris: List<Uri>) {
        Thread {
            val (files, left) = Shared.take(this, uris)
            main.post {
                if (left.isNotEmpty()) Toast.makeText(this, getString(R.string.share_left_out, left.joinToString()), Toast.LENGTH_LONG).show()
                page("shareFiles(${JSONObject.quote(text.take(50_000))}, ${files})")
            }
        }.start()
    }

    /** Have the page run `js`, now or once it's loaded. */
    private fun page(js: String) {
        if (loaded) web.evaluateJavascript(js, null) else pending.add(js)
    }

    override fun onResume() {
        super.onResume()
        Rettui.setShowing(this, true)
        watch.run()
        if (started) askNotifications()
    }

    override fun onPause() {
        Rettui.setShowing(this, false)
        main.removeCallbacks(watch)
        super.onPause()
    }

    /**
     * While the web UI is on screen: that rettui is still there. Stopped
     * with Stop, the app says so; stopped otherwise (a backup restored, or
     * a crash), it waits for rettui to be back.
     */
    private val watch = object : Runnable {
        override fun run() {
            main.removeCallbacks(this)
            if (web.visibility != android.view.View.VISIBLE) return
            if (!loaded || Rettui.running(this@MainActivity)) {
                followPageTheme()
                main.postDelayed(this, 2000)
                return
            }
            web.visibility = android.view.View.GONE
            if (Rettui.stoppedByYou(this@MainActivity)) return stopped()
            waiting.visibility = android.view.View.VISIBLE
            waitedSince = System.currentTimeMillis()
            waitForRettui(grace = 20_000)
        }
    }

    /**
     * Back, from Android 13 on (the app takes part in its predictive back,
     * so the screens it opens slide back to this one).
     */
    private val back = if (Build.VERSION.SDK_INT >= 33) OnBackInvokedCallback { goBack() } else null

    @Deprecated("Activity's own back handling, kept for Android 12 and earlier")
    override fun onBackPressed() = goBack()

    private fun goBack() {
        if (web.visibility != android.view.View.VISIBLE) {
            moveTaskToBack(true)
            return
        }
        // The page first: it closes what's open on top, or goes back a pane.
        web.evaluateJavascript("window.rettuiBack ? rettuiBack() : false") { handled ->
            if (handled == "true") return@evaluateJavascript
            if (web.canGoBack()) web.goBack() else moveTaskToBack(true)
        }
    }

    private fun begin() {
        started = true
        RettuiService.start(this)
        waitedSince = System.currentTimeMillis()
        waitForRettui()
        if (!askBattery()) askNotifications()
    }

    /** Past the first start's questions (notifications, battery). */
    private var started = false
    /** The question on screen, so another doesn't go over it. */
    private var asking: AlertDialog? = null

    /**
     * With notifications off for rettui (or its Messages ones), messages
     * arrive unannounced while the app is closed: say so once, with a way to
     * turn them on. Asked again only after they've been on.
     */
    private fun askNotifications() {
        val prefs = getSharedPreferences("app", MODE_PRIVATE)
        val manager = getSystemService(android.app.NotificationManager::class.java)
        val messages = manager.getNotificationChannel(RettuiService.CHANNEL_MESSAGES)
        val on = manager.areNotificationsEnabled() && messages?.importance != android.app.NotificationManager.IMPORTANCE_NONE
        if (on) {
            prefs.edit().remove("asked_notifications").apply()
            return
        }
        if (prefs.getBoolean("asked_notifications", false) || asking?.isShowing == true) return
        prefs.edit().putBoolean("asked_notifications", true).apply()
        asking = dialog()
            .setTitle(R.string.notifications_off)
            .setMessage(R.string.notifications_off_note)
            .setPositiveButton(R.string.notifications_turn_on) { _, _ ->
                // The Messages ones, if only they are off; else the app's.
                val settings = if (manager.areNotificationsEnabled() && messages != null) {
                    Intent(Settings.ACTION_CHANNEL_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_CHANNEL_ID, messages.id)
                } else {
                    Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                }
                runCatching { startActivity(settings.putExtra(Settings.EXTRA_APP_PACKAGE, packageName)) }
            }
            .setNegativeButton(R.string.battery_not_now, null)
            .show()
    }

    /**
     * Once: ask to be left out of battery optimisation, which on many
     * phones stops background apps, and with them Reticulum and messages.
     */
    private fun askBattery(): Boolean {
        val prefs = getSharedPreferences("app", MODE_PRIVATE)
        if (getSystemService(PowerManager::class.java).isIgnoringBatteryOptimizations(packageName) || prefs.getBoolean("asked_battery", false)) return false
        prefs.edit().putBoolean("asked_battery", true).apply()
        asking = dialog()
            .setTitle(R.string.battery)
            .setMessage(R.string.battery_note)
            .setPositiveButton(R.string.battery_allow) { _, _ ->
                runCatching {
                    @SuppressLint("BatteryLife")
                    val ask = Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:$packageName"))
                    startActivity(ask)
                }
            }
            .setNegativeButton(R.string.battery_not_now, null)
            .show()
        return true
    }

    /**
     * Load the web UI once rettui answers; if it isn't there `grace`
     * milliseconds on (it failed, or was stopped), say so.
     */
    private fun waitForRettui(grace: Long = 3000) {
        Thread {
            while (true) {
                val token = RettuiService.token(this)
                if (token != null && RettuiService.listening(this)) {
                    main.post { show(token) }
                    return@Thread
                }
                // It never started, or stopped: the service went.
                if (!Rettui.running(this) && System.currentTimeMillis() - waitedSince > grace) {
                    main.post { stopped() }
                    return@Thread
                }
                Thread.sleep(250)
            }
        }.start()
    }

    private fun show(token: String) {
        waiting.visibility = android.view.View.GONE
        web.visibility = android.view.View.VISIBLE
        // Loaded again after rettui was away: its page starts over.
        loaded = false
        web.loadUrl("http://127.0.0.1:${Rettui.port(this)}/?token=$token")
        main.postDelayed(watch, 2000)
    }

    /** rettui isn't running: stopped with Stop, or it failed (its log shown). */
    private fun stopped() {
        val byYou = Rettui.stoppedByYou(this)
        val log = if (byYou) "" else RettuiService.logFile(this).takeIf { it.isFile }?.readText()?.takeLast(20_000) ?: ""
        val palette = palette()
        val content = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(16), dp(16), dp(16), dp(16))
        }
        content.addView(TextView(this).apply {
            text = getString(if (byYou) R.string.stopped_by_you else R.string.stopped)
            textSize = 20f
            setTypeface(typeface, android.graphics.Typeface.BOLD)
            setTextColor(palette.text)
            setPadding(0, dp(8), 0, dp(8))
        })
        content.primary(getString(if (byYou) R.string.start else R.string.restart)) {
            // Start over, without what the app was first opened for.
            intent = Intent(this@MainActivity, MainActivity::class.java)
            recreate()
        }
        // A backup can be restored, or the identity saved, even with
        // rettui failing to start.
        content.secondary(getString(R.string.backups)) { startActivity(Intent(this@MainActivity, BackupsActivity::class.java)) }
        content.secondary(getString(R.string.close_app)) { finishAndRemoveTask() }
        if (log.isNotEmpty()) {
            val card = ScrollView(this).apply {
                background = android.graphics.drawable.GradientDrawable().apply {
                    setColor(palette.panel)
                    setStroke(dp(1), palette.border)
                    cornerRadius = dp(12).toFloat()
                }
                setPadding(dp(12), dp(12), dp(12), dp(12))
                addView(TextView(context).apply {
                    text = log
                    typeface = android.graphics.Typeface.MONOSPACE
                    textSize = 11f
                    setTextColor(palette.dim)
                    setTextIsSelectable(true)
                })
            }
            content.addView(card, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f).apply { topMargin = dp(16) })
        }
        showClearOfBars(content)
    }

    /** The web UI's screen, with a new WebView if its last one went (see onRenderProcessGone). */
    private fun showWebScreen() {
        (waiting.parent as? ViewGroup)?.removeView(waiting)
        screen = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            addView(waiting, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
            addView(web, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
        }
        showClearOfBars(screen)
        pageColors()?.let { (background, light) -> paint(background, light) }
    }

    /**
     * The web UI's theme, as the page last had it: its background, and
     * whether it's light. Kept, so the next start shows it from the first.
     */
    private fun pageColors(): Pair<Int, Boolean>? {
        val prefs = getSharedPreferences("app", MODE_PRIVATE)
        if (!prefs.contains("page_background")) return null
        return prefs.getInt("page_background", BACKGROUND) to prefs.getBoolean("page_light", false)
    }

    /**
     * Match the bars around the page to its theme (Status, Theme): the room
     * kept for the status and navigation bars has the page's background, and
     * the bars' icons read on it. Asked when the page loads and every few
     * seconds while it shows (see watch), since its theme can change.
     */
    private fun followPageTheme() {
        if (!loaded) return
        val ask = "(() => { const root = document.documentElement; " +
            "return [root.dataset.theme || 'dark', getComputedStyle(root).getPropertyValue('--bg').trim()]; })()"
        web.evaluateJavascript(ask) { said ->
            val (theme, color) = runCatching { org.json.JSONArray(said).let { it.getString(0) to it.getString(1) } }.getOrNull()
                ?: return@evaluateJavascript
            // The page's own colour, only if it's a plain #rrggbb.
            val background = Regex("#[0-9a-fA-F]{6}").matchEntire(color)?.let { Color.parseColor(it.value) } ?: return@evaluateJavascript
            val light = theme == "light"
            if (pageColors() == background to light) return@evaluateJavascript
            getSharedPreferences("app", MODE_PRIVATE).edit().putInt("page_background", background).putBoolean("page_light", light).apply()
            paint(background, light)
        }
    }

    private fun paint(background: Int, light: Boolean) {
        // Not over another screen (rettui stopped).
        if (screen.parent == null) return
        screen.setBackgroundColor(background)
        web.setBackgroundColor(background)
        (waiting.getChildAt(1) as? TextView)?.setTextColor(if (light) Color.DKGRAY else Color.LTGRAY)
        colorBars(background, light)
    }

    /** The Android permission behind a page's request for the microphone or the camera. */
    private fun mediaPermission(resource: String) =
        if (resource == PermissionRequest.RESOURCE_VIDEO_CAPTURE) Manifest.permission.CAMERA else Manifest.permission.RECORD_AUDIO

    private fun local(uri: Uri) = uri.scheme == "http" && uri.host == "127.0.0.1" && uri.port == Rettui.port(this)

    private fun setUpWebView() {
        // Only a debug build's page can be inspected from a computer (chrome://inspect).
        if (applicationInfo.flags and android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE != 0) WebView.setWebContentsDebuggingEnabled(true)
        web.setBackgroundColor(BACKGROUND)
        web.settings.apply {
            javaScriptEnabled = true
            domStorageEnabled = true
            mediaPlaybackRequiresUserGesture = false
            setGeolocationEnabled(true)
            allowFileAccess = false
            allowContentAccess = false
            userAgentString = "$userAgentString $AGENT"
        }
        CookieManager.getInstance().setAcceptCookie(true)
        web.webViewClient = object : WebViewClient() {
            // Files shared from other apps, which the page fetches (see Shared).
            override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? {
                val url = request.url
                if (!local(url) || url.path?.startsWith(Shared.PATH) != true) return null
                return Shared.serve(this@MainActivity, url.path!!) ?: WebResourceResponse("text/plain", null, 404, "Not Found", null, null)
            }

            // Anything not rettui's opens in the phone's browser.
            override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                if (local(request.url)) return false
                SCREENS[request.url.toString()]?.let {
                    startActivity(Intent(this@MainActivity, it))
                    return true
                }
                try {
                    startActivity(Intent(Intent.ACTION_VIEW, request.url))
                } catch (e: Exception) {
                    Toast.makeText(this@MainActivity, getString(R.string.no_app_for_link, request.url.scheme), Toast.LENGTH_SHORT).show()
                }
                return true
            }

            override fun onPageFinished(view: WebView, url: String) {
                loaded = true
                pending.forEach { view.evaluateJavascript(it, null) }
                pending.clear()
                followPageTheme()
            }

            // The page's renderer went (Android ends it to free memory, or it
            // crashed). Left alone, Android ends the whole app with it; a new
            // WebView loads the web UI again instead.
            override fun onRenderProcessGone(view: WebView, detail: RenderProcessGoneDetail): Boolean {
                if (view != web) return true
                Log.w("rettui", "The web UI's renderer went (crashed: ${detail.didCrash()}); loading it again")
                val showing = web.visibility == android.view.View.VISIBLE
                main.removeCallbacks(watch)
                (web.parent as? ViewGroup)?.removeView(web)
                web.destroy()
                loaded = false
                web = WebView(this@MainActivity).apply { visibility = android.view.View.GONE }
                setUpWebView()
                showWebScreen()
                if (showing) {
                    waiting.visibility = android.view.View.VISIBLE
                    waitedSince = System.currentTimeMillis()
                    waitForRettui(grace = 20_000)
                }
                return true
            }
        }
        web.webChromeClient = object : WebChromeClient() {
            // The microphone (voice messages) and the camera (scanning a QR code), for rettui's own page.
            override fun onPermissionRequest(request: PermissionRequest) {
                val wanted = request.resources.toList()
                val allowed = setOf(PermissionRequest.RESOURCE_AUDIO_CAPTURE, PermissionRequest.RESOURCE_VIDEO_CAPTURE)
                if (!local(request.origin) || wanted.isEmpty() || !allowed.containsAll(wanted)) return request.deny()
                val needed = wanted.map { mediaPermission(it) }.filter { checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }
                if (needed.isEmpty()) return request.grant(request.resources)
                pendingPermission?.deny()
                pendingPermission = request
                requestPermissions(needed.toTypedArray(), ASK_MICROPHONE)
            }

            override fun onGeolocationPermissionsShowPrompt(origin: String, callback: GeolocationPermissions.Callback) {
                if (!local(Uri.parse(origin))) return callback.invoke(origin, false, false)
                if (checkSelfPermission(Manifest.permission.ACCESS_FINE_LOCATION) == PackageManager.PERMISSION_GRANTED ||
                    checkSelfPermission(Manifest.permission.ACCESS_COARSE_LOCATION) == PackageManager.PERMISSION_GRANTED
                ) {
                    return callback.invoke(origin, true, false)
                }
                pendingLocation = origin to callback
                requestPermissions(arrayOf(Manifest.permission.ACCESS_FINE_LOCATION, Manifest.permission.ACCESS_COARSE_LOCATION), ASK_LOCATION)
            }

            override fun onShowFileChooser(view: WebView, callback: ValueCallback<Array<Uri>>, params: FileChooserParams): Boolean {
                pendingFiles?.onReceiveValue(null)
                pendingFiles = callback
                return try {
                    startActivityForResult(withCamera(params), PICK_FILES)
                    true
                } catch (e: Exception) {
                    pendingFiles = null
                    false
                }
            }
        }
        web.setDownloadListener { url, _, disposition, mime, _ -> download(url, disposition, mime) }
    }

    /** The photo the camera was asked to take, if it was offered (see withCamera). */
    private var pendingPhoto: Pair<File, Uri>? = null

    /**
     * The page's file picker, with the camera beside it when the page takes
     * pictures: a photo taken goes in the app's private cache (see Photos),
     * not the gallery.
     */
    private fun withCamera(params: WebChromeClient.FileChooserParams): Intent {
        pendingPhoto = null
        val picker = params.createIntent()
        val types = params.acceptTypes.orEmpty().map { it.trim().lowercase() }.filter { it.isNotEmpty() }
        val pictures = types.isEmpty() || types.any { it == "*/*" || it.startsWith("image/") || it in setOf(".jpg", ".jpeg") }
        val camera = Intent(MediaStore.ACTION_IMAGE_CAPTURE)
        if (!pictures || camera.resolveActivity(packageManager) == null) return picker
        val (file, uri) = Photos.next(this)
        camera.putExtra(MediaStore.EXTRA_OUTPUT, uri)
            .addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION or Intent.FLAG_GRANT_READ_URI_PERMISSION)
        // The camera app has to be let in to that one photo; ClipData carries the grant.
        camera.clipData = android.content.ClipData.newRawUri(null, uri)
        pendingPhoto = file to uri
        return Intent.createChooser(picker, null).putExtra(Intent.EXTRA_INITIAL_INTENTS, arrayOf(camera))
    }

    /** A download waiting for the "save as" screen: its address, name and type. */
    private var pendingSave: Triple<String, String, String>? = null

    /**
     * Save what the web UI offers to download (an export, a backup, a file
     * from a message) where Settings, Where files are saved, says: asked
     * for each time, in the folder picked, or in Downloads. rettui's own
     * pages only: the request carries the login.
     */
    private fun download(url: String, disposition: String?, mime: String?) {
        val uri = Uri.parse(url)
        if (!local(uri)) return
        val name = URLUtil.guessFileName(url, disposition, mime)
        val type = mime ?: "application/octet-stream"
        if (DownloadsActivity.asks(this)) {
            pendingSave = Triple(url, name, type)
            try {
                startActivityForResult(
                    Intent(Intent.ACTION_CREATE_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE).setType(type).putExtra(Intent.EXTRA_TITLE, name),
                    SAVE_FILE,
                )
                return
            } catch (e: Exception) {
                pendingSave = null
            }
        }
        save(url, name, type, null)
    }

    /** Fetch `url` and write it to `target` (the file asked for), else the folder picked, else Downloads. */
    private fun save(url: String, name: String, type: String, target: Uri?) {
        val cookie = CookieManager.getInstance().getCookie(url)
        Toast.makeText(this, getString(R.string.downloading, name), Toast.LENGTH_SHORT).show()
        Thread {
            val result = try {
                val connection = URL(url).openConnection() as HttpURLConnection
                cookie?.let { connection.setRequestProperty("Cookie", it) }
                RettuiService.token(this)?.let { connection.setRequestProperty("Authorization", "Bearer $it") }
                connection.inputStream.use { input ->
                    val folder = DownloadsActivity.folder(this)
                    if (target != null) {
                        contentResolver.openOutputStream(target)!!.use { input.copyTo(it) }
                        getString(R.string.downloads_saved, name)
                    } else if (folder != null) {
                        val parent = DocumentsContract.buildDocumentUriUsingTree(folder, DocumentsContract.getTreeDocumentId(folder))
                        val saved = DocumentsContract.createDocument(contentResolver, parent, type, name) ?: error(getString(R.string.downloads_no_file))
                        contentResolver.openOutputStream(saved)!!.use { input.copyTo(it) }
                        getString(R.string.downloads_saved, name)
                    } else if (Build.VERSION.SDK_INT >= 29) {
                        val values = ContentValues().apply {
                            put(MediaStore.Downloads.DISPLAY_NAME, name)
                            put(MediaStore.Downloads.MIME_TYPE, type)
                        }
                        val saved = contentResolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
                            ?: error(getString(R.string.downloads_no_room))
                        contentResolver.openOutputStream(saved)!!.use { input.copyTo(it) }
                        getString(R.string.downloads_saved_in, name, getString(R.string.downloads_folder))
                    } else {
                        val dir = getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS) ?: filesDir
                        File(dir, name).outputStream().use { input.copyTo(it) }
                        getString(R.string.downloads_saved_in, name, dir.path)
                    }
                }
            } catch (e: Exception) {
                getString(R.string.download_failed, name, e.message)
            }
            main.post { Toast.makeText(this, result, Toast.LENGTH_LONG).show() }
        }.start()
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<String>, results: IntArray) {
        val granted = results.isNotEmpty() && results.any { it == PackageManager.PERMISSION_GRANTED }
        when (requestCode) {
            ASK_START -> begin()
            ASK_MICROPHONE -> pendingPermission?.let { asked ->
                // Only what was allowed: a camera refused doesn't take the microphone with it.
                val allowedNow = asked.resources.filter { checkSelfPermission(mediaPermission(it)) == PackageManager.PERMISSION_GRANTED }
                if (allowedNow.isEmpty()) asked.deny() else asked.grant(allowedNow.toTypedArray())
            }.also { pendingPermission = null }
            ASK_LOCATION -> pendingLocation?.let { (origin, callback) -> callback.invoke(origin, granted, false) }
                .also { pendingLocation = null }
        }
    }

    @Deprecated("Activity's own result handling, kept for API 26")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        if (requestCode == PICK_FILES) {
            // A photo taken comes back with nothing picked: it's in the file given to the camera.
            val photo = pendingPhoto?.takeIf { (file, _) -> resultCode == RESULT_OK && data?.data == null && data?.clipData == null && file.length() > 0 }
            pendingFiles?.onReceiveValue(if (photo != null) arrayOf(photo.second) else WebChromeClient.FileChooserParams.parseResult(resultCode, data))
            pendingFiles = null
            pendingPhoto = null
            return
        }
        if (requestCode == SAVE_FILE) {
            val (url, name, type) = pendingSave ?: return
            pendingSave = null
            val target = data?.data
            if (resultCode == RESULT_OK && target != null) save(url, name, type, target) else Toast.makeText(this, R.string.not_saved, Toast.LENGTH_SHORT).show()
            return
        }
        super.onActivityResult(requestCode, resultCode, data)
    }

    // Out of sight, the page does nothing: its timers stop, and it counts as
    // hidden, so it fetches nothing until it shows again (rettui's service
    // shows notifications meanwhile).
    override fun onStop() {
        web.onPause()
        web.pauseTimers()
        // Nor does it watch that rettui is there (asking Android every 2 s).
        main.removeCallbacks(watch)
        super.onStop()
    }

    override fun onStart() {
        super.onStart()
        web.resumeTimers()
        web.onResume()
        if (web.visibility == android.view.View.VISIBLE) main.postDelayed(watch, 2000)
    }

    override fun onDestroy() {
        web.destroy()
        super.onDestroy()
    }
}
