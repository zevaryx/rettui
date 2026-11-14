package org.zevaryx.rettui

import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.graphics.Color
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.text.InputType
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.RadioButton
import android.widget.RadioGroup
import android.view.View
import android.widget.TextView
import java.io.File
import org.json.JSONObject
import java.text.DateFormat
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
        private const val PICK_FOLDER = 5
    }

    private lateinit var scheduleFolder: TextView
    private lateinit var scheduleSealed: TextView
    private lateinit var scheduleLast: TextView

    private val main = Handler(Looper.getMainLooper())
    private var busy: AlertDialog? = null
    /** The passphrase for the backup being saved, until it's made ("": none). */
    private var backupPassphrase = ""

    override fun onCreate(savedInstanceState: Bundle?) {
        applyPageTheme()
        super.onCreate(savedInstanceState)
        val page = Page(this, getString(R.string.backups))

        val identity = page.section(getString(R.string.identity))
        identity.note(getString(R.string.identity_note))
        identity.primary(getString(R.string.identity_save)) {
            save(SAVE_IDENTITY, "application/octet-stream", "rettui-identity")
        }
        identity.secondary(getString(R.string.identity_bring)) { open(OPEN_IDENTITY) }

        val backup = page.section(getString(R.string.backup))
        backup.note(getString(R.string.backup_note))
        backup.primary(getString(R.string.backup_save)) {
            // A passphrase first (or none), then where.
            askPassphrase(R.string.backup_passphrase, getString(R.string.backup_passphrase_note), twice = true) { passphrase ->
                backupPassphrase = passphrase
                val day = SimpleDateFormat("yyyy-MM-dd", Locale.ROOT).format(Date())
                save(SAVE_BACKUP, "application/gzip", "rettui-backup-$day.tar.gz")
            }
        }
        backup.secondary(getString(R.string.backup_restore)) { open(OPEN_BACKUP) }

        val schedule = page.section(getString(R.string.schedule))
        schedule.note(getString(R.string.schedule_note, ScheduledBackups.KEPT))
        scheduleFolder = schedule.value()
        schedule.secondary(getString(R.string.downloads_choose)) {
            startActivityForResult(
                Intent(Intent.ACTION_OPEN_DOCUMENT_TREE).addFlags(
                    Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION or Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION,
                ),
                PICK_FOLDER,
            )
        }
        val every = listOf(0 to R.string.schedule_off, 1 to R.string.schedule_daily, 7 to R.string.schedule_weekly)
        val buttons = mutableMapOf<Int, RadioButton>()
        schedule.addView(RadioGroup(this).apply {
            setPadding(0, dp(8), 0, 0)
            for ((days, label) in every) {
                addView(RadioButton(context).apply {
                    id = View.generateViewId()
                    text = getString(label)
                    tinted()
                    minHeight = dp(48)
                    isChecked = ScheduledBackups.every(context) == days
                    buttons[days] = this
                    setOnClickListener {
                        val was = ScheduledBackups.every(context)
                        if (days == 0 || ScheduledBackups.decided(context)) {
                            ScheduledBackups.setEvery(context, days)
                            showSchedule()
                        } else {
                            // Before the first: whether to seal them, and with what.
                            askSchedulePassphrase(then = {
                                ScheduledBackups.setEvery(context, days)
                                showSchedule()
                            }, cancelled = { buttons[was]?.isChecked = true })
                        }
                    }
                })
            }
        })
        scheduleSealed = schedule.note("")
        schedule.secondary(getString(R.string.schedule_change_passphrase)) { askSchedulePassphrase(then = { showSchedule() }, cancelled = {}) }
        scheduleLast = schedule.note("")
        schedule.primary(getString(R.string.schedule_now)) {
            if (ScheduledBackups.decided(this)) makeOneNow() else askSchedulePassphrase(then = { makeOneNow() }, cancelled = {})
        }
        showSchedule()
    }

    private fun makeOneNow() = work {
        val problem = ScheduledBackups.run(this)
        main.post { showSchedule() }
        if (problem == null) Rettui.Said(true, getString(R.string.schedule_saved)) else Rettui.Said(false, problem)
    }

    /**
     * Ask for the scheduled backups' passphrase (none: not sealed), and keep
     * the key made from it; `then` once it's kept.
     */
    private fun askSchedulePassphrase(then: () -> Unit, cancelled: () -> Unit) {
        askPassphrase(R.string.schedule_passphrase, getString(R.string.schedule_passphrase_note), twice = true, cancelled = cancelled) { passphrase ->
            work {
                val problem = ScheduledBackups.setPassphrase(this, passphrase)
                main.post { if (problem == null) then() else cancelled() }
                when {
                    problem != null -> Rettui.Said(false, problem)
                    passphrase.isEmpty() -> Rettui.Said(true, getString(R.string.schedule_not_sealed))
                    else -> Rettui.Said(true, getString(R.string.schedule_sealed))
                }
            }
        }
    }

    /**
     * Ask for a passphrase: a new one, `twice` (to check it), which may be
     * left empty for none; otherwise the one to open a backup with.
     */
    private fun askPassphrase(title: Int, note: String, twice: Boolean, cancelled: () -> Unit = {}, then: (String) -> Unit) {
        fun field(hint: Int) = EditText(this).apply {
            setHint(hint)
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
        }
        val first = field(if (twice) R.string.passphrase_none else R.string.passphrase)
        val again = field(R.string.passphrase_again)
        // A new one easy to guess: said, as rettui says it, and theirs to use anyway.
        val weak = TextView(this).apply {
            setTextColor(Color.rgb(if (palette().light) 0x8a else 0xf2, if (palette().light) 0x5a else 0xd4, if (palette().light) 0x00 else 0x5c))
            setPadding(0, dp(4), 0, dp(4))
            visibility = View.GONE
        }
        if (twice) {
            first.addTextChangedListener(object : android.text.TextWatcher {
                override fun beforeTextChanged(s: CharSequence?, start: Int, count: Int, after: Int) {}
                override fun onTextChanged(s: CharSequence?, start: Int, before: Int, count: Int) {}
                override fun afterTextChanged(s: android.text.Editable?) {
                    val why = weakness(s?.toString().orEmpty())
                    weak.text = why?.let { getString(it) }
                    weak.visibility = if (why == null) View.GONE else View.VISIBLE
                }
            })
        }
        val layout = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(20), dp(8), dp(20), 0)
            addView(TextView(context).apply {
                text = note
                setPadding(0, 0, 0, dp(8))
            })
            addView(first)
            if (twice) {
                addView(weak)
                addView(again)
            }
        }
        val dialog = dialog()
            .setTitle(title)
            .setView(layout)
            .setPositiveButton(android.R.string.ok, null)
            .setNegativeButton(android.R.string.cancel) { _, _ -> cancelled() }
            .setOnCancelListener { cancelled() }
            .show()
        // Kept open when they differ, saying so.
        dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener {
            val typed = first.text.toString()
            when {
                twice && typed != again.text.toString() -> again.error = getString(R.string.passphrases_differ)
                !twice && typed.isEmpty() -> first.error = getString(R.string.passphrase_needed)
                else -> {
                    dialog.dismiss()
                    then(typed)
                }
            }
        }
    }

    /** Why a new passphrase is easy to guess, if it is (rettui's own rule, backup::sealed::weakness). */
    private fun weakness(passphrase: String): Int? {
        val length = passphrase.codePointCount(0, passphrase.length)
        return when {
            length == 0 -> null
            length < 20 && passphrase.all { it in '0'..'9' } -> R.string.passphrase_pin
            length < 12 -> R.string.passphrase_short
            else -> null
        }
    }

    /** Where scheduled backups go, and how the last one went. */
    private fun showSchedule() {
        val folder = ScheduledBackups.folder(this)
        scheduleFolder.text = if (folder == null) {
            getString(R.string.schedule_no_folder)
        } else {
            getString(R.string.downloads_in_folder, Uri.decode(android.provider.DocumentsContract.getTreeDocumentId(folder)))
        }
        scheduleSealed.text = when {
            !ScheduledBackups.decided(this) -> ""
            ScheduledBackups.sealed(this) -> getString(R.string.schedule_sealed)
            else -> getString(R.string.schedule_not_sealed)
        }
        val last = ScheduledBackups.last(this)
        scheduleLast.text = when {
            last == null -> ""
            last.second == null -> getString(R.string.schedule_last_ok, DateFormat.getDateTimeInstance().format(Date(last.first)))
            else -> getString(R.string.schedule_last_failed, DateFormat.getDateTimeInstance().format(Date(last.first)), last.second)
        }
        // Nothing to say: no gap either.
        scheduleSealed.visibility = if (scheduleSealed.text.isEmpty()) View.GONE else View.VISIBLE
        scheduleLast.visibility = if (scheduleLast.text.isEmpty()) View.GONE else View.VISIBLE
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
            PICK_FOLDER -> {
                ScheduledBackups.setFolder(this, uri)
                showSchedule()
            }
            SAVE_IDENTITY -> work { saveThrough(uri) { file -> Rettui.command(this, "identity", "export", file.path) }.also { if (it.ok) markBackedUp() } }
            SAVE_BACKUP -> {
                val passphrase = backupPassphrase.also { backupPassphrase = "" }
                val sealing = if (passphrase.isEmpty()) "--no-passphrase" else "--passphrase"
                work { saveThrough(uri) { file -> Rettui.withPassphrase(this, passphrase, "backup", file.path, sealing) } }
            }
            OPEN_IDENTITY -> askName { name -> work { openThrough(uri) { file -> Rettui.command(this, "identity", "import", file.path, "--name", name) } } }
            OPEN_BACKUP -> dialog()
                .setTitle(R.string.backup_restore)
                .setMessage(R.string.restore_confirm)
                .setPositiveButton(R.string.restore) { _, _ -> restoreChosen(uri) }
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
            Rettui.Said(false, getString(R.string.save_failed, e.message))
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
            Rettui.Said(false, getString(R.string.read_file_failed, e.message))
        } finally {
            file.delete()
        }
    }

    /** A backup to restore: its passphrase first, if it's sealed with one. */
    private fun restoreChosen(uri: Uri) {
        Thread {
            val sealed = runCatching { contentResolver.openInputStream(uri)!!.use { Rettui.sealed(it) } }.getOrDefault(false)
            main.post {
                if (!sealed) return@post work { restore(uri, null) }
                askPassphrase(R.string.restore_passphrase, getString(R.string.restore_passphrase_note), twice = false) { passphrase ->
                    work { restore(uri, passphrase) }
                }
            }
        }.start()
    }

    /** Restoring needs rettui stopped; it starts again after, either way. */
    private fun restore(uri: Uri, passphrase: String?): Rettui.Said {
        if (!RettuiService.stopAndWait(this)) {
            return Rettui.Said(false, getString(R.string.restore_not_stopped))
        }
        val said = openThrough(uri) { file -> Rettui.withPassphrase(this, passphrase, "restore", "--force", file.path) }
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
        Rettui.api(this, "identity/backed-up", JSONObject())
    }

    private fun askName(then: (String) -> Unit) {
        val input = EditText(this).apply {
            hint = getString(R.string.identity_name_hint)
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_CAP_WORDS
        }
        dialog()
            .setTitle(R.string.identity_name)
            .setView(input)
            .setPositiveButton(R.string.keep) { _, _ -> then(input.text.toString()) }
            .setNegativeButton(android.R.string.cancel, null)
            .show()
    }

    /** Run `job` away from the screen, then say what came of it. */
    private fun work(job: () -> Rettui.Said) {
        busy = dialog().setMessage(R.string.working).setCancelable(false).show()
        Thread {
            val said = job()
            main.post {
                busy?.dismiss()
                busy = null
                dialog()
                    .setTitle(if (said.ok) R.string.done else R.string.failed)
                    .setMessage(said.text.removePrefix("Error: ").ifEmpty { getString(if (said.ok) R.string.done else R.string.failed) })
                    .setPositiveButton(android.R.string.ok, null)
                    .show()
            }
        }.start()
    }
}
