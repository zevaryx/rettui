package org.zevaryx.rettui

import android.app.Activity
import android.content.Context
import android.graphics.Color
import android.os.Build
import android.view.View
import android.view.WindowInsets
import android.view.WindowInsetsController
import kotlin.math.roundToInt

/** The app's own screens' background, the web UI's dark one (its `--bg`). */
val BACKGROUND = Color.rgb(0x16, 0x16, 0x1e)

/** `value` density-independent pixels, in this screen's pixels. */
fun Context.dp(value: Int): Int = (value * resources.displayMetrics.density).roundToInt()

/**
 * Show `root` as the screen, kept clear of the status and navigation bars,
 * the camera cutout and the keyboard. Android 15 draws apps behind those
 * whatever they ask; from Android 11 on this app asks the same everywhere,
 * so it looks the same, and pads `root` itself (on top of its own padding).
 */
fun Activity.showClearOfBars(root: View) {
    val palette = palette()
    root.setBackgroundColor(palette.background)
    setContentView(root)
    colorBars(palette.background, palette.light)
    if (Build.VERSION.SDK_INT < 30) return
    window.setDecorFitsSystemWindows(false)
    val own = intArrayOf(root.paddingLeft, root.paddingTop, root.paddingRight, root.paddingBottom)
    root.setOnApplyWindowInsetsListener { view, insets ->
        val bars = insets.getInsets(WindowInsets.Type.systemBars() or WindowInsets.Type.displayCutout() or WindowInsets.Type.ime())
        view.setPadding(own[0] + bars.left, own[1] + bars.top, own[2] + bars.right, own[3] + bars.bottom)
        // Used up here: the web UI makes room for them too (its CSS's
        // safe-area insets), which would leave the room twice.
        WindowInsets.CONSUMED
    }
    root.requestApplyInsets()
}

/**
 * The status and navigation bars over `background`: their icons dark on a
 * `light` one, light otherwise. Before Android 11 the bars aren't drawn
 * over the screen, so they're given the colour too.
 */
@Suppress("DEPRECATION")
fun Activity.colorBars(background: Int, light: Boolean) {
    if (Build.VERSION.SDK_INT >= 30) {
        val both = WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS or WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS
        window.insetsController?.setSystemBarsAppearance(if (light) both else 0, both)
        return
    }
    window.statusBarColor = background
    window.navigationBarColor = background
    val flags = View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR or View.SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR
    val decor = window.decorView
    decor.systemUiVisibility = if (light) decor.systemUiVisibility or flags else decor.systemUiVisibility and flags.inv()
}
