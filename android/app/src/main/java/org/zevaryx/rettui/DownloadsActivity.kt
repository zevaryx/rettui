package org.zevaryx.rettui

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.provider.DocumentsContract
import android.widget.CheckBox
import android.widget.TextView

/**
 * Where files the web UI offers to download are saved (a NomadNet file, an
 * export, a backup): in Downloads, in a folder picked here with Android's
 * own folder picker, or asked for each time with its "save as" screen.
 */
class DownloadsActivity : Activity() {
    companion object {
        private const val PICK_FOLDER = 1
        private const val PREFS = "app"
        private const val ASK = "downloads_ask"
        private const val FOLDER = "downloads_folder"

        /** Whether each file asks where it goes. */
        fun asks(context: Context) = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(ASK, false)

        /** The folder picked, if there is one and Android still lets the app use it. */
        fun folder(context: Context): Uri? {
            val saved = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(FOLDER, null) ?: return null
            val uri = Uri.parse(saved)
            val held = context.contentResolver.persistedUriPermissions.any { it.uri == uri && it.isWritePermission }
            return uri.takeIf { held }
        }
    }

    private lateinit var where: TextView

    override fun onCreate(savedInstanceState: Bundle?) {
        applyPageTheme()
        super.onCreate(savedInstanceState)
        val page = Page(this, getString(R.string.downloads))
        val card = page.section()
        card.note(getString(R.string.downloads_note))
        card.addView(CheckBox(this).apply {
            text = getString(R.string.downloads_ask)
            tinted()
            isChecked = asks(this@DownloadsActivity)
            setOnCheckedChangeListener { _, checked ->
                getSharedPreferences(PREFS, MODE_PRIVATE).edit().putBoolean(ASK, checked).apply()
            }
        })
        val folder = page.section()
        where = folder.value()
        folder.primary(getString(R.string.downloads_choose)) {
            startActivityForResult(
                Intent(Intent.ACTION_OPEN_DOCUMENT_TREE).addFlags(
                    Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION or Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION,
                ),
                PICK_FOLDER,
            )
        }
        folder.secondary(getString(R.string.downloads_default)) {
            // Kept if scheduled backups go there too.
            folder(this@DownloadsActivity)?.takeIf { it != ScheduledBackups.folder(this@DownloadsActivity) }?.let { runCatching { contentResolver.releasePersistableUriPermission(it, Intent.FLAG_GRANT_WRITE_URI_PERMISSION or Intent.FLAG_GRANT_READ_URI_PERMISSION) } }
            getSharedPreferences(PREFS, MODE_PRIVATE).edit().remove(FOLDER).apply()
            show()
        }
        show()
    }

    private fun show() {
        val folder = folder(this)
        where.text = if (folder == null) {
            getString(R.string.downloads_in_downloads)
        } else {
            getString(R.string.downloads_in_folder, Uri.decode(DocumentsContract.getTreeDocumentId(folder)))
        }
    }

    @Deprecated("Activity's own result handling, kept for API 26")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        val uri = data?.data
        if (requestCode == PICK_FOLDER && resultCode == RESULT_OK && uri != null) {
            contentResolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
            getSharedPreferences(PREFS, MODE_PRIVATE).edit().putString(FOLDER, uri.toString()).apply()
            show()
            return
        }
        super.onActivityResult(requestCode, resultCode, data)
    }
}
