package org.zevaryx.rettui

import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.graphics.Color
import android.graphics.Typeface
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.text.InputType
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * The identity and backups, which the web UI leaves alone (anyone with its
 * login link could otherwise copy the identity): save a copy of the
 * identity, keep another from a file, and save or restore a full backup.
 * Each goes through rettui's own commands, with the file passing through
 * the app's private cache, removed straight after.
 */
class BackupsActivity : Activity() {
    companion object {
        private const val SAVE_IDENTITY = 1
        private const val OPEN_IDENTITY = 2
        private const val SAVE_BACKUP = 3
        private const val OPEN_BACKUP = 4
    }

    private val main = Handler(Looper.getMainLooper())
    private var busy: AlertDialog? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        title = getString(R.string.backups)
        val column = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(48, 48, 48, 48)
        }
        fun heading(text: String) = column.addView(TextView(this).apply {
            this.text = text
            textSize = 20f
            setTypeface(typeface, Typeface.BOLD)
            setTextColor(Color.WHITE)
            setPadding(0, 32, 0, 8)
        })
        fun note(text: String) = column.addView(TextView(this).apply {
            this.text = text
            textSize = 14f
            setTextColor(Color.LTGRAY)
            setPadding(0, 0, 0, 12)
        })
        fun action(text: String, run: () -> Unit) = column.addView(Button(this).apply {
            this.text = text
            isAllCaps = false
            setOnClickListener { run() }
        })

        heading(getString(R.string.identity))
        note(getString(R.string.identity_note))
        action(getString(R.string.identity_save)) {
            save(SAVE_IDENTITY, "application/octet-stream", "rettui-identity")
        }
        action(getString(R.string.identity_bring)) { open(OPEN_IDENTITY) }

        heading(getString(R.string.backup))
        note(getString(R.string.backup_note))
        action(getString(R.string.backup_save)) {
            val day = SimpleDateFormat("yyyy-MM-dd", Locale.ROOT).format(Date())
            save(SAVE_BACKUP, "application/gzip", "rettui-backup-$day.tar.gz")
        }
        action(getString(R.string.backup_restore)) { open(OPEN_BACKUP) }

        setContentView(ScrollView(this).apply {
            setBackgroundColor(Color.rgb(0x16, 0x18, 0x1d))
            addView(column)
        })
    }

    private fun save(request: Int, type: String, name: String) {
        startActivityForResult(
            Intent(Intent.ACTION_CREATE_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE).setType(type).putExtra(Intent.EXTRA_TITLE, name),
            request,
        )
    }

    private fun open(request: Int) {
        startActivityForResult(Intent(Intent.ACTION_OPEN_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE).setType("*/*"), request)
    }

    @Deprecated("Activity's own result handling, kept for API 26")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        val uri = data?.data
        if (resultCode != RESULT_OK || uri == null) return
        when (requestCode) {
            SAVE_IDENTITY -> work { saveThrough(uri) { file -> Rettui.command(this, "identity", "export", file.path) }.also { if (it.ok) markBackedUp() } }
            SAVE_BACKUP -> work { saveThrough(uri) { file -> Rettui.command(this, "backup", file.path) } }
            OPEN_IDENTITY -> askName { name -> work { openThrough(uri) { file -> Rettui.command(this, "identity", "import", file.path, "--name", name) } } }
            OPEN_BACKUP -> AlertDialog.Builder(this)
                .setTitle(R.string.backup_restore)
                .setMessage(R.string.restore_confirm)
                .setPositiveButton(R.string.restore) { _, _ -> work { restore(uri) } }
                .setNegativeButton(android.R.string.cancel, null)
                .show()
        }
    }

    /** A file rettui writes, then copied to where the user chose. */
    private fun saveThrough(uri: Uri, make: (File) -> Rettui.Said): Rettui.Said {
        val file = scratch()
        return try {
            val said = make(file)
            if (said.ok) {
                contentResolver.openOutputStream(uri, "wt")!!.use { out -> file.inputStream().use { it.copyTo(out) } }
            } else {
                deleteChosen(uri)
            }
            said
        } catch (e: Exception) {
            deleteChosen(uri)
            Rettui.Said(false, "Couldn't save it: ${e.message}")
        } finally {
            file.delete()
        }
    }

    /** The file the user chose, copied for rettui to read. */
    private fun openThrough(uri: Uri, use: (File) -> Rettui.Said): Rettui.Said {
        val file = scratch()
        return try {
            contentResolver.openInputStream(uri)!!.use { input -> file.outputStream().use { input.copyTo(it) } }
            use(file)
        } catch (e: Exception) {
            Rettui.Said(false, "Couldn't read it: ${e.message}")
        } finally {
            file.delete()
        }
    }

    /** Restoring needs rettui stopped; it starts again after, either way. */
    private fun restore(uri: Uri): Rettui.Said {
        if (!RettuiService.stopAndWait(this)) {
            return Rettui.Said(false, getString(R.string.restore_not_stopped))
        }
        val said = openThrough(uri) { file -> Rettui.command(this, "restore", "--force", file.path) }
        RettuiService.start(this)
        return said
    }

    /** A new, empty file in the app's private cache, for one file passing through. */
    private fun scratch(): File = File.createTempFile("passing", null, cacheDir).also { it.delete() }

    /** The file made for a save that failed: it's empty, so it goes. */
    private fun deleteChosen(uri: Uri) {
        try {
            android.provider.DocumentsContract.deleteDocument(contentResolver, uri)
        } catch (e: Exception) {
        }
    }

    /** Tell rettui the identity is backed up (its first steps say so). */
    private fun markBackedUp() {
        val token = RettuiService.token(this) ?: return
        try {
            val connection = URL("http://127.0.0.1:${RettuiService.PORT}/api/identity/backed-up").openConnection() as HttpURLConnection
            connection.requestMethod = "POST"
            connection.setRequestProperty("Authorization", "Bearer $token")
            connection.connectTimeout = 2000
            connection.responseCode
            connection.disconnect()
        } catch (e: Exception) {
        }
    }

    private fun askName(then: (String) -> Unit) {
        val input = EditText(this).apply {
            hint = getString(R.string.identity_name_hint)
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_CAP_WORDS
        }
        AlertDialog.Builder(this)
            .setTitle(R.string.identity_name)
            .setView(input)
            .setPositiveButton(R.string.keep) { _, _ -> then(input.text.toString()) }
            .setNegativeButton(android.R.string.cancel, null)
            .show()
    }

    /** Run `job` away from the screen, then say what came of it. */
    private fun work(job: () -> Rettui.Said) {
        busy = AlertDialog.Builder(this).setMessage(R.string.working).setCancelable(false).show()
        Thread {
            val said = job()
            main.post {
                busy?.dismiss()
                busy = null
                AlertDialog.Builder(this)
                    .setTitle(if (said.ok) R.string.done else R.string.failed)
                    .setMessage(said.text.removePrefix("Error: ").ifEmpty { getString(if (said.ok) R.string.done else R.string.failed) })
                    .setPositiveButton(android.R.string.ok, null)
                    .show()
            }
        }.start()
    }
}
