package org.zevaryx.rettui

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Path
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import kotlin.math.abs

/**
 * A contact's picture for a notification: LXMFace's face for their address,
 * the same one the web UI shows (`/face/<address>.svg`, an 8x8 grid of
 * coloured squares in a circle), drawn here since Android can't read an SVG.
 * Kept in the cache once drawn.
 */
object Avatars {
    private const val SIZE = 96
    private val ADDRESS = Regex("[0-9a-f]{32}")
    private val CELL = Regex("""<rect x="(\d+)" y="(\d+)"[^>]*fill="hsl\(([\d.]+),\s*([\d.]+)%,\s*([\d.]+)%\)"""")

    /** The face for `address`, or null if it can't be had (rettui not up, or not an address). */
    fun face(context: Context, address: String): Bitmap? {
        if (!ADDRESS.matches(address)) return null
        val file = File(File(context.cacheDir, "faces"), "$address.png")
        if (file.isFile) android.graphics.BitmapFactory.decodeFile(file.path)?.let { return it }
        val svg = fetch(context, address) ?: return null
        val bitmap = draw(svg) ?: return null
        runCatching {
            file.parentFile?.mkdirs()
            file.outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
        }
        return bitmap
    }

    private fun fetch(context: Context, address: String): String? = try {
        val token = RettuiService.token(context) ?: error("no token")
        val connection = URL("http://127.0.0.1:${Rettui.port(context)}/face/$address.svg").openConnection() as HttpURLConnection
        connection.setRequestProperty("Authorization", "Bearer $token")
        connection.connectTimeout = 2000
        connection.readTimeout = 4000
        try {
            if (connection.responseCode in 200..299) connection.inputStream.bufferedReader().readText().take(100_000) else null
        } finally {
            connection.disconnect()
        }
    } catch (e: Exception) {
        null
    }

    /** The squares of the face, in a circle. */
    private fun draw(svg: String): Bitmap? {
        val cells = CELL.findAll(svg).toList()
        if (cells.isEmpty()) return null
        val bitmap = Bitmap.createBitmap(SIZE, SIZE, Bitmap.Config.ARGB_8888)
        val canvas = Canvas(bitmap)
        canvas.clipPath(Path().apply { addCircle(SIZE / 2f, SIZE / 2f, SIZE / 2f, Path.Direction.CW) })
        val paint = Paint().apply { isAntiAlias = false }
        val side = SIZE / 8f
        for (cell in cells) {
            val (x, y, h, s, l) = cell.destructured
            paint.color = hsl(h.toFloat(), s.toFloat() / 100f, l.toFloat() / 100f)
            val left = x.toInt() * side
            val top = y.toInt() * side
            canvas.drawRect(left, top, left + side + 1, top + side + 1, paint)
        }
        return bitmap
    }

    /** An HSL colour (degrees, and fractions) as Android's. */
    private fun hsl(h: Float, s: Float, l: Float): Int {
        val c = (1 - abs(2 * l - 1)) * s
        val x = c * (1 - abs((h / 60f) % 2 - 1))
        val m = l - c / 2
        val (r, g, b) = when {
            h < 60 -> Triple(c, x, 0f)
            h < 120 -> Triple(x, c, 0f)
            h < 180 -> Triple(0f, c, x)
            h < 240 -> Triple(0f, x, c)
            h < 300 -> Triple(x, 0f, c)
            else -> Triple(c, 0f, x)
        }
        fun channel(v: Float) = ((v + m) * 255f).toInt().coerceIn(0, 255)
        return Color.rgb(channel(r), channel(g), channel(b))
    }
}
