package org.zevaryx.rettui

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.job.JobInfo
import android.app.job.JobParameters
import android.app.job.JobScheduler
import android.app.job.JobService
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.DocumentsContract
import android.util.Log
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * Full backups on a schedule (Identity and backups, Backups on a schedule):
 * every day or week, while the phone charges, rettui's `backup` into a
 * folder picked with Android's folder picker, keeping the newest few. Only
 * the backups it made itself are ever deleted there.
 */
class ScheduledBackups : JobService() {
    companion object {
        private const val TAG = "rettui"
        private const val JOB = 1
        private const val PREFS = "app"
        private const val EVERY = "backups_every_days"
        private const val FOLDER = "backups_folder"
        private const val LAST = "backups_last"
        private const val LAST_OK = "backups_last_ok"
        private const val CHANNEL = "backups"
        private const val ID_FAILED = 2

        /** Its backups' names: `rettui-backup-auto-<date>-<time>.tar.gz`. */
        private const val PREFIX = "rettui-backup-auto-"
        private const val SUFFIX = ".tar.gz"

        /** The newest this many are kept. */
        const val KEPT = 5

        private fun prefs(context: Context) = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

        /** Every how many days (1, 7), or 0: off. */
        fun every(context: Context) = prefs(context).getInt(EVERY, 0)

        /** The folder picked, if there is one and Android still lets the app use it. */
        fun folder(context: Context): Uri? {
            val saved = prefs(context).getString(FOLDER, null) ?: return null
            val uri = Uri.parse(saved)
            return uri.takeIf { context.contentResolver.persistedUriPermissions.any { it.uri == uri && it.isWritePermission } }
        }

        /** The last one: when (Unix ms), and whether it worked, or what went wrong. */
        fun last(context: Context): Pair<Long, String?>? {
            val prefs = prefs(context)
            if (!prefs.contains(LAST)) return null
            return prefs.getLong(LAST, 0) to prefs.getString(LAST_OK, null)
        }

        fun setFolder(context: Context, folder: Uri) {
            val flags = Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
            // The folder before, unless the downloads use it too.
            folder(context)?.takeIf { it != folder && it != DownloadsActivity.folder(context) }
                ?.let { runCatching { context.contentResolver.releasePersistableUriPermission(it, flags) } }
            context.contentResolver.takePersistableUriPermission(folder, flags)
            prefs(context).edit().putString(FOLDER, folder.toString()).apply()
            schedule(context)
        }

        fun setEvery(context: Context, days: Int) {
            prefs(context).edit().putInt(EVERY, days).apply()
            schedule(context)
        }

        /** Ask Android to run it as set (or not at all), from now on and after a restart. */
        fun schedule(context: Context) {
            val jobs = context.getSystemService(JobScheduler::class.java)
            val days = every(context)
            if (days <= 0 || folder(context) == null) {
                jobs.cancel(JOB)
                return
            }
            val interval = days * 24L * 60 * 60 * 1000
            val pending = jobs.getPendingJob(JOB)
            if (pending != null && pending.intervalMillis == interval) return
            jobs.schedule(
                JobInfo.Builder(JOB, ComponentName(context, ScheduledBackups::class.java))
                    .setPeriodic(interval)
                    // While it charges: it reads all of rettui's files.
                    .setRequiresCharging(true)
                    .setPersisted(true)
                    .build(),
            )
        }

        /** Make one now, and say what came of it (null: it worked). */
        fun run(context: Context): String? {
            val folder = folder(context) ?: return context.getString(R.string.schedule_no_folder)
            val file = File.createTempFile("passing", null, context.cacheDir).also { it.delete() }
            val problem = try {
                val said = Rettui.command(context, "backup", file.path)
                if (!said.ok) {
                    said.text.removePrefix("Error: ").ifEmpty { context.getString(R.string.failed) }
                } else {
                    val resolver = context.contentResolver
                    val parent = DocumentsContract.buildDocumentUriUsingTree(folder, DocumentsContract.getTreeDocumentId(folder))
                    val name = PREFIX + SimpleDateFormat("yyyy-MM-dd-HHmm", Locale.ROOT).format(Date()) + SUFFIX
                    val saved = DocumentsContract.createDocument(resolver, parent, "application/gzip", name)
                        ?: error(context.getString(R.string.downloads_no_file))
                    try {
                        resolver.openOutputStream(saved, "wt")!!.use { out -> file.inputStream().use { it.copyTo(out) } }
                    } catch (e: Exception) {
                        runCatching { DocumentsContract.deleteDocument(resolver, saved) }
                        throw e
                    }
                    prune(context, folder)
                    // rettui's first steps stop asking for one.
                    Rettui.api(context, "identity/backed-up", org.json.JSONObject())
                    null
                }
            } catch (e: Exception) {
                e.message ?: e.toString()
            } finally {
                file.delete()
            }
            prefs(context).edit().putLong(LAST, System.currentTimeMillis()).putString(LAST_OK, problem).apply()
            return problem
        }

        /** Delete its own backups there past the newest [KEPT] (their names sort by time). */
        private fun prune(context: Context, folder: Uri) {
            val resolver = context.contentResolver
            val children = DocumentsContract.buildChildDocumentsUriUsingTree(folder, DocumentsContract.getTreeDocumentId(folder))
            val ours = mutableListOf<Pair<String, String>>()
            resolver.query(children, arrayOf(DocumentsContract.Document.COLUMN_DOCUMENT_ID, DocumentsContract.Document.COLUMN_DISPLAY_NAME), null, null, null)?.use {
                while (it.moveToNext()) {
                    val name = it.getString(1) ?: continue
                    if (name.startsWith(PREFIX) && name.endsWith(SUFFIX)) ours.add(it.getString(0) to name)
                }
            }
            for ((id, name) in ours.sortedByDescending { it.second }.drop(KEPT)) {
                runCatching { DocumentsContract.deleteDocument(resolver, DocumentsContract.buildDocumentUriUsingTree(folder, id)) }
                    .onFailure { Log.i(TAG, "Couldn't delete the old backup $name: $it") }
            }
        }
    }

    override fun onStartJob(params: JobParameters): Boolean {
        Thread {
            val problem = run(this)
            if (problem != null) failed(problem)
            jobFinished(params, false)
        }.start()
        return true
    }

    // Stopped part way (the phone unplugged): the next one is soon enough.
    override fun onStopJob(params: JobParameters) = false

    /** Say it didn't work, so the backups don't stop unnoticed. */
    private fun failed(problem: String) {
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel(CHANNEL, getString(R.string.channel_backups), NotificationManager.IMPORTANCE_DEFAULT))
        val open = PendingIntent.getActivity(this, 0, Intent(this, BackupsActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
        manager.notify(
            ID_FAILED,
            Notification.Builder(this, CHANNEL)
                .setSmallIcon(R.drawable.ic_notification)
                .setColor(getColor(R.color.accent))
                .setContentTitle(getString(R.string.schedule_failed))
                .setContentText(problem.take(300))
                .setStyle(Notification.BigTextStyle().bigText(problem.take(1000)))
                .setContentIntent(open)
                .setAutoCancel(true)
                .build(),
        )
    }
}
