package org.zevaryx.rettui

import android.Manifest
import android.app.Activity
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
import android.provider.MediaStore
import android.view.Gravity
import android.view.ViewGroup
import android.webkit.CookieManager
import android.webkit.GeolocationPermissions
import android.webkit.PermissionRequest
import android.webkit.URLUtil
import android.webkit.ValueCallback
import android.webkit.WebChromeClient
import android.webkit.WebResourceRequest
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

        /** The web UI's link to the app's identity and backups screen. */
        const val BACKUPS_LINK = "rettui-app:backups"

        /** What a notification tapped is about (JSON, as the web UI has it). */
        const val EXTRA_TARGET = "target"
        private const val ASK_START = 1
        private const val ASK_MICROPHONE = 2
        private const val ASK_LOCATION = 3
        private const val PICK_FILES = 4

        /** Whether the web UI is on screen (it shows notifications then). */
        @Volatile
        var visible = false
            private set
    }

    private val main = Handler(Looper.getMainLooper())
    private lateinit var web: WebView
    private lateinit var waiting: TextView
    private var loaded = false
    private var target: String? = null
    private var waitedSince = 0L
    private var pendingPermission: PermissionRequest? = null
    private var pendingLocation: Pair<String, GeolocationPermissions.Callback>? = null
    private var pendingFiles: ValueCallback<Array<Uri>>? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        target = intent.getStringExtra(EXTRA_TARGET)
        waiting = TextView(this).apply {
            text = getString(R.string.starting)
            gravity = Gravity.CENTER
            textSize = 18f
            setTextColor(Color.LTGRAY)
            setBackgroundColor(Color.rgb(0x16, 0x18, 0x1d))
        }
        web = WebView(this).apply { visibility = android.view.View.GONE }
        setUpWebView()
        setContentView(LinearLayout(this).apply {
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
        intent.getStringExtra(EXTRA_TARGET)?.let { opened ->
            if (loaded) openTarget(opened) else target = opened
        }
    }

    override fun onResume() {
        super.onResume()
        visible = true
    }

    override fun onPause() {
        visible = false
        super.onPause()
    }

    @Deprecated("Activity's own back handling, kept for API 26")
    override fun onBackPressed() {
        if (web.visibility == android.view.View.VISIBLE && web.canGoBack()) web.goBack() else moveTaskToBack(true)
    }

    private fun begin() {
        RettuiService.start(this)
        waitedSince = System.currentTimeMillis()
        waitForRettui()
    }

    /** Load the web UI once rettui answers; if it stops, say so. */
    private fun waitForRettui() {
        Thread {
            while (true) {
                val token = RettuiService.token(this)
                if (token != null && RettuiService.listening()) {
                    main.post { show(token) }
                    return@Thread
                }
                // It never started, or stopped: the service went.
                if (!RettuiService.running && System.currentTimeMillis() - waitedSince > 3000) {
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
        web.loadUrl("http://127.0.0.1:${RettuiService.PORT}/?token=$token")
    }

    private fun stopped() {
        val log = RettuiService.logFile(this).takeIf { it.isFile }?.readText()?.takeLast(20_000) ?: ""
        setContentView(LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(Color.rgb(0x16, 0x18, 0x1d))
            setPadding(32, 48, 32, 32)
            addView(TextView(context).apply {
                text = getString(R.string.stopped)
                textSize = 18f
                setTextColor(Color.WHITE)
            })
            addView(Button(context).apply {
                text = getString(R.string.restart)
                setOnClickListener { recreate() }
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

    private fun openTarget(json: String) {
        // Only JSON reaches the page: quoted as a string, then parsed there.
        web.evaluateJavascript("try { openTarget(JSON.parse(${JSONObject.quote(json)})) } catch (e) { console.warn(e) }", null)
    }

    private fun local(uri: Uri) = uri.scheme == "http" && uri.host == "127.0.0.1" && uri.port == RettuiService.PORT

    private fun setUpWebView() {
        web.setBackgroundColor(Color.rgb(0x16, 0x18, 0x1d))
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
            // Anything not rettui's opens in the phone's browser.
            override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                if (local(request.url)) return false
                if (request.url.toString() == BACKUPS_LINK) {
                    startActivity(Intent(this@MainActivity, BackupsActivity::class.java))
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
                target?.let {
                    target = null
                    openTarget(it)
                }
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

    override fun onDestroy() {
        web.destroy()
        super.onDestroy()
    }
}
