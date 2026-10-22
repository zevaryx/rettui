package org.zevaryx.rettui

import android.app.Activity
import android.graphics.Color
import android.os.Build
import android.view.View
import android.view.WindowInsets

/** The app's own screens' background, the web UI's dark one. */
val BACKGROUND = Color.rgb(0x16, 0x18, 0x1d)

/**
 * Show `root` as the screen, kept clear of the status and navigation bars,
 * the camera cutout and the keyboard. Android 15 draws apps behind those
 * whatever they ask; from Android 11 on this app asks the same everywhere,
 * so it looks the same, and pads `root` itself (on top of its own padding).
 */
fun Activity.showClearOfBars(root: View) {
    root.setBackgroundColor(BACKGROUND)
    setContentView(root)
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
