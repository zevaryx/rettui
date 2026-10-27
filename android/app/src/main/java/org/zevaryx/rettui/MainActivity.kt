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
import android.provider.MediaStore
import android.provider.Settings
import android.view.Gravity
import android.view.ViewGroup
import android.webkit.CookieManager
import android.webkit.GeolocationPermissions
import android.webkit.PermissionRequest
import android.webkit.URLUtil
import android.webkit.ValueCallback
import android.webkit.WebChromeClient
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.Button
import android.widget.LinearLayout
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
        private val SCREENS = mapOf("rettui-app:backups" to BackupsActivity::class.java, "rettui-app:rnodes" to RNodesActivity::class.java)

        /** What a notification tapped is about (JSON, as the web UI has it). */
        const val EXTRA_TARGET = "target"
        private const val ASK_START = 1
        private const val ASK_MICROPHONE = 2
        private const val ASK_LOCATION = 3
        private const val PICK_FILES = 4

        /** Links other apps may open with rettui (see AndroidManifest.xml). */
        private val SCHEMES = setOf("lxma", "lxmf", "nomadnetwork", "rrc")
    }

    private val main = Handler(Looper.getMainLooper())
    private lateinit var web: WebView
    private lateinit var waiting: TextView
    private var loaded = false
    /** What the page is to do once it has loaded (a notification's, a share's). */
    private val pending = mutableListOf<String>()
    private var waitedSince = 0L
    private var pendingPermission: PermissionRequest? = null
    private var pendingLocation: Pair<String, GeolocationPermissions.Callback>? = null
    private var pendingFiles: ValueCallback<Array<Uri>>? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        handle(intent)
        waiting = TextView(this).apply {
            text = getString(R.string.starting)
            gravity = Gravity.CENTER
            textSize = 18f
            setTextColor(Color.LTGRAY)
        }
        web = WebView(this).apply { visibility = android.view.View.GONE }
        setUpWebView()
        showClearOfBars(LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            addView(waiting, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
            addView(web, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
        })
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

    @Deprecated("Activity's own back handling, kept for API 26")
    override fun onBackPressed() {
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
        RettuiService.start(this)
        waitedSince = System.currentTimeMillis()
        waitForRettui()
        askBattery()
    }

    /**
     * Once: ask to be left out of battery optimisation, which on many
     * phones stops background apps, and with them Reticulum and messages.
     */
    private fun askBattery() {
        val prefs = getSharedPreferences("app", MODE_PRIVATE)
        if (getSystemService(PowerManager::class.java).isIgnoringBatteryOptimizations(packageName) || prefs.getBoolean("asked_battery", false)) return
        prefs.edit().putBoolean("asked_battery", true).apply()
        AlertDialog.Builder(this)
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
        showClearOfBars(LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(32, 48, 32, 32)
            addView(TextView(context).apply {
                text = getString(if (byYou) R.string.stopped_by_you else R.string.stopped)
                textSize = 18f
                setTextColor(Color.WHITE)
            })
            addView(Button(context).apply {
                text = getString(if (byYou) R.string.start else R.string.restart)
                setOnClickListener {
                    // Start over, without what the app was first opened for.
                    intent = Intent(this@MainActivity, MainActivity::class.java)
                    recreate()
                }
            })
            addView(Button(context).apply {
                text = getString(R.string.close_app)
                setOnClickListener { finishAndRemoveTask() }
            })
            // A backup can be restored, or the identity saved, even with
            // rettui failing to start.
            addView(Button(context).apply {
                text = getString(R.string.backups)
                setOnClickListener { startActivity(Intent(this@MainActivity, BackupsActivity::class.java)) }
            })
            addView(ScrollView(context).apply {
                addView(TextView(context).apply {
                    text = log
                    typeface = android.graphics.Typeface.MONOSPACE
                    textSize = 11f
                    setTextColor(Color.LTGRAY)
                    setTextIsSelectable(true)
                })
            }, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f))
        })
    }

    private fun local(uri: Uri) = uri.scheme == "http" && uri.host == "127.0.0.1" && uri.port == Rettui.port(this)

    private fun setUpWebView() {
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
                    Toast.makeText(this@MainActivity, "Nothing here opens ${request.url.scheme} links", Toast.LENGTH_SHORT).show()
                }
                return true
            }

            override fun onPageFinished(view: WebView, url: String) {
                loaded = true
                pending.forEach { view.evaluateJavascript(it, null) }
                pending.clear()
            }
        }
        web.webChromeClient = object : WebChromeClient() {
            override fun onPermissionRequest(request: PermissionRequest) {
                if (!local(request.origin) || request.resources.toList() != listOf(PermissionRequest.RESOURCE_AUDIO_CAPTURE)) {
                    request.deny()
                } else if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) {
                    request.grant(request.resources)
                } else {
                    pendingPermission?.deny()
                    pendingPermission = request
                    requestPermissions(arrayOf(Manifest.permission.RECORD_AUDIO), ASK_MICROPHONE)
                }
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
                    startActivityForResult(params.createIntent(), PICK_FILES)
                    true
                } catch (e: Exception) {
                    pendingFiles = null
                    false
                }
            }
        }
        web.setDownloadListener { url, _, disposition, mime, _ -> download(url, disposition, mime) }
    }

    /**
     * Save what the web UI offers to download (an export, a backup, a file
     * from a message) in Downloads. rettui's own pages only: the request
     * carries the login.
     */
    private fun download(url: String, disposition: String?, mime: String?) {
        val uri = Uri.parse(url)
        if (!local(uri)) return
        val name = URLUtil.guessFileName(url, disposition, mime)
        val cookie = CookieManager.getInstance().getCookie(url)
        Toast.makeText(this, "Downloading $name", Toast.LENGTH_SHORT).show()
        Thread {
            val result = try {
                val connection = URL(url).openConnection() as HttpURLConnection
                cookie?.let { connection.setRequestProperty("Cookie", it) }
                RettuiService.token(this)?.let { connection.setRequestProperty("Authorization", "Bearer $it") }
                connection.inputStream.use { input ->
                    if (Build.VERSION.SDK_INT >= 29) {
                        val values = ContentValues().apply {
                            put(MediaStore.Downloads.DISPLAY_NAME, name)
                            put(MediaStore.Downloads.MIME_TYPE, mime ?: "application/octet-stream")
                        }
                        val saved = contentResolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
                            ?: error("no room in Downloads")
                        contentResolver.openOutputStream(saved)!!.use { input.copyTo(it) }
                        "Saved $name in Downloads"
                    } else {
                        val dir = getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS) ?: filesDir
                        File(dir, name).outputStream().use { input.copyTo(it) }
                        "Saved $name in ${dir.path}"
                    }
                }
            } catch (e: Exception) {
                "Couldn't download $name: ${e.message}"
            }
            main.post { Toast.makeText(this, result, Toast.LENGTH_LONG).show() }
        }.start()
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<String>, results: IntArray) {
        val granted = results.isNotEmpty() && results.any { it == PackageManager.PERMISSION_GRANTED }
        when (requestCode) {
            ASK_START -> begin()
            ASK_MICROPHONE -> pendingPermission?.let { if (granted) it.grant(it.resources) else it.deny() }
                .also { pendingPermission = null }
            ASK_LOCATION -> pendingLocation?.let { (origin, callback) -> callback.invoke(origin, granted, false) }
                .also { pendingLocation = null }
        }
    }

    @Deprecated("Activity's own result handling, kept for API 26")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        if (requestCode == PICK_FILES) {
            pendingFiles?.onReceiveValue(WebChromeClient.FileChooserParams.parseResult(resultCode, data))
            pendingFiles = null
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
