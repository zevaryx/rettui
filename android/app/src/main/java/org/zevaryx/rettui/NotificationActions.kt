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
 * Reply and Mark read on a notification about a conversation or a room, and Accept and Decline on one about a game's challenge:
 * done through the web UI's API, as the page itself does them, then the
 * notification goes (rettui says it's read, and the service closes it).
 */
class NotificationActions : BroadcastReceiver() {
    companion object {
        const val ACTION_REPLY = "org.zevaryx.rettui.REPLY"
        const val ACTION_READ = "org.zevaryx.rettui.READ"
        const val ACTION_ACCEPT = "org.zevaryx.rettui.ACCEPT"
        const val ACTION_DECLINE = "org.zevaryx.rettui.DECLINE"
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
                val said = act(context, target, text, intent.action)
                val manager = context.getSystemService(NotificationManager::class.java)
                if (said.ok) {
                    if (text != null) {
                        // Android keeps a notification that was answered from itself, spinner and all,
                        // and ignores a cancel of it: it's replaced by what was sent, which goes by itself.
                        manager.notify(
                            tag, 0,
                            Notification.Builder(context, RettuiService.CHANNEL_MESSAGES)
                                .setSmallIcon(R.drawable.ic_notification)
                                .setColor(context.getColor(R.color.accent))
                                .setContentTitle(context.getString(R.string.reply_sent))
                                .setContentText(text.take(300))
                                .setCategory(Notification.CATEGORY_MESSAGE)
                                .setGroup(RettuiService.GROUP)
                                .setOnlyAlertOnce(true)
                                .setTimeoutAfter(3000)
                                .build(),
                        )
                        // The sent notification goes by itself (a timeout): the stack under its summary
                        // is counted again once it has.
                        Thread.sleep(3500)
                    } else {
                        manager.cancel(tag, 0)
                        Thread.sleep(500)
                    }
                    RettuiService.refreshSummary(context)
                } else {
                    // Kept, saying why, so the reply can be written again.
                    manager.notify(
                        tag, 0,
                        Notification.Builder(context, RettuiService.CHANNEL_MESSAGES)
                            .setSmallIcon(R.drawable.ic_notification)
                            .setColor(context.getColor(R.color.accent))
                            .setContentTitle(
                                context.getString(
                                    when {
                                        text != null -> R.string.reply_failed
                                        intent.action == ACTION_ACCEPT || intent.action == ACTION_DECLINE -> R.string.game_failed
                                        else -> R.string.read_failed
                                    },
                                ),
                            )
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
    private fun act(context: Context, target: JSONObject, text: String?, action: String?): Rettui.Said {
        return when (target.optString("kind")) {
            // A challenge accepted or declined, as the Games tab's buttons do.
            "game" -> {
                val command = if (action == ACTION_ACCEPT) "accept" else "decline"
                val app = Uri.encode(target.optString("app"))
                val session = Uri.encode(target.optString("session"))
                Rettui.api(context, "games/$app/$session", JSONObject().put("command", command))
            }
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
            else -> Rettui.Said(false, context.getString(R.string.nothing_to_do))
        }
    }
}
