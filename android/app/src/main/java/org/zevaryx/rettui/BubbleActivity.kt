package org.zevaryx.rettui

/**
 * The app's screen in a bubble: a conversation floating over other apps
 * (Android 11 and later, when its notification offers one). It's the same
 * screen as [MainActivity], opened on that conversation; Android needs it
 * to be an activity of its own, in a task of its own.
 *
 * While it's open, reading the conversation (which opening it does) doesn't
 * take its notification, and so the bubble, away: see RettuiService.
 */
class BubbleActivity : MainActivity() {
    override fun onResume() {
        super.onResume()
        Rettui.setBubbleShowing(this, true)
    }

    override fun onPause() {
        Rettui.setBubbleShowing(this, false)
        super.onPause()
    }
}
