package org.zevaryx.rettui

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import android.webkit.WebResourceResponse
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.UUID

/**
 * Files shared with rettui from other apps, on their way to the web UI:
 * copied into the app's private cache, then served to the page at
 * `/__android/shared/<id>` by the WebView itself (never by rettui, and to
 * nothing but this app's page), for it to attach to a message.
 */
object Shared {
    const val PATH = "/__android/shared/"

    /** At most this much at once, which a message can still carry to rettui. */
    private const val MOST_BYTES = 40L * 1024 * 1024

    private fun folder(context: Context) = File(context.cacheDir, "shared")

    /**
     * Copy `uris` in (what was shared before goes), and say what the page
     * fetches: `[{url, name, type}]`; and what was left out, if any.
     */
    fun take(context: Context, uris: List<Uri>): Pair<JSONArray, List<String>> {
        val dir = folder(context)
        dir.deleteRecursively()
        dir.mkdirs()
        val files = JSONArray()
        val left = mutableListOf<String>()
        var total = 0L
        for (uri in uris) {
            val name = displayName(context, uri)
            try {
                val id = UUID.randomUUID().toString()
                val copy = File(dir, id)
                val copied = context.contentResolver.openInputStream(uri)?.use { input ->
                    copy.outputStream().use { output -> input.copyTo(output) }
                } ?: 0L
                if (total + copied > MOST_BYTES) {
                    copy.delete()
                    left.add(name)
                    continue
                }
                total += copied
                val type = context.contentResolver.getType(uri) ?: "application/octet-stream"
                File(dir, "$id.type").writeText(type)
                files.put(JSONObject().put("url", PATH + id).put("name", name).put("type", type))
            } catch (e: Exception) {
                left.add(name)
            }
        }
        return files to left
    }

    /** The file shared as `path`, for the WebView to answer the page with. */
    fun serve(context: Context, path: String): WebResourceResponse? {
        val id = path.removePrefix(PATH)
        // Only ids this made: nothing else of the app's files.
        if (runCatching { UUID.fromString(id) }.isFailure) return null
        val file = File(folder(context), id)
        if (!file.isFile) return null
        val type = File(folder(context), "$id.type").takeIf { it.isFile }?.readText() ?: "application/octet-stream"
        return WebResourceResponse(type, null, file.inputStream())
    }

    /** Its name as the sharing app gave it, with nothing path-like in it. */
    private fun displayName(context: Context, uri: Uri): String {
        val named = runCatching {
            context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
                if (cursor.moveToFirst()) cursor.getString(0) else null
            }
        }.getOrNull() ?: uri.lastPathSegment ?: "file"
        return named.substringAfterLast('/').replace(Regex("[\\\\:*?\"<>|]"), "_").take(200).ifBlank { "file" }
    }
}
