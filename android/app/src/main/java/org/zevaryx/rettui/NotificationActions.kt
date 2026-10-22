package org.zevaryx.rettui

import android.app.Notification
import android.app.NotificationManager
import android.app.RemoteInput
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.net.Uri
import org.json.JSONObject

/**
 * Reply and Mark read on a notification about a conversation or a room:
 * done through the web UI's API, as the page itself does them, then the
 * notification goes (rettui says it's read, and the service closes it).
 */
class NotificationActions : BroadcastReceiver() {
    companion object {
        const val ACTION_REPLY = "org.zevaryx.rettui.REPLY"
        const val ACTION_READ = "org.zevaryx.rettui.READ"
        const val EXTRA_TARGET = "target"
        const val EXTRA_TAG = "tag"
        const val REPLY = "reply"
    }

    override fun onReceive(context: Context, intent: Intent) {
        val target = runCatching { JSONObject(intent.getStringExtra(EXTRA_TARGET) ?: "") }.getOrNull() ?: return
        val tag = intent.getStringExtra(EXTRA_TAG) ?: return
        val text = if (intent.action == ACTION_REPLY) {
            RemoteInput.getResultsFromIntent(intent)?.getCharSequence(REPLY)?.toString()?.trim().orEmpty().ifEmpty { return }
        } else {
            null
        }
        val done = goAsync()
        Thread {
            try {
                val said = act(context, target, text)
                val manager = context.getSystemService(NotificationManager::class.java)
                if (said.ok) {
                    manager.cancel(tag, 0)
                } else {
                    // Kept, saying why, so the reply can be written again.
                    manager.notify(
                        tag, 0,
                        Notification.Builder(context, RettuiService.CHANNEL_MESSAGES)
                            .setSmallIcon(R.drawable.ic_notification)
                            .setContentTitle(context.getString(if (text != null) R.string.reply_failed else R.string.read_failed))
                            .setContentText(said.text.take(300))
                            .setAutoCancel(true)
                            .build(),
                    )
                }
            } finally {
                done.finish()
            }
        }.start()
    }

    /** Send `text` there (if any), then mark it read. */
    private fun act(context: Context, target: JSONObject, text: String?): Rettui.Said {
        return when (target.optString("kind")) {
            "conversation" -> {
                val key = Uri.encode(target.optString("key"))
                if (text != null) {
                    // The way of delivery set for this conversation, as the page uses.
                    val conversation = Rettui.api(context, "conversations/$key")
                    val mode = runCatching { JSONObject(conversation.text).getJSONObject("contact").getString("delivery") }.getOrNull()
                        ?.takeIf { it in setOf("auto", "direct", "propagated") } ?: "auto"
                    val sent = Rettui.api(context, "conversations/$key/send", JSONObject().put("content", text).put("mode", mode))
                    if (!sent.ok) return sent
                }
                Rettui.api(context, "conversations/$key/read", JSONObject())
            }
            "room" -> {
                val hub = Uri.encode(target.optString("hub"))
                val room = target.optString("room")
                if (text != null) {
                    val sent = Rettui.api(context, "channels/$hub/send", JSONObject().put("room", room).put("text", text))
                    if (!sent.ok) return sent
                }
                Rettui.api(context, "channels/$hub/read", JSONObject().put("room", room))
            }
            else -> Rettui.Said(false, "Nothing to do for this notification")
        }
    }
}
