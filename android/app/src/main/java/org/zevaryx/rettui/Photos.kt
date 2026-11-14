package org.zevaryx.rettui

import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import java.io.File

/**
 * A photo taken for a message (+, then the camera): the camera app writes
 * it here, into the app's private cache, rather than into the phone's
 * gallery, and the page attaches it from here. Only apps this app hands a
 * photo's address to may open it, and only that photo.
 */
class Photos : ContentProvider() {
    companion object {
        private fun folder(context: Context) = File(context.cacheDir, "photos")

        /** A new, empty photo for the camera to fill, and its address; those taken before go. */
        fun next(context: Context): Pair<File, Uri> {
            val dir = folder(context)
            dir.deleteRecursively()
            dir.mkdirs()
            val name = "photo-${System.currentTimeMillis()}.jpg"
            return File(dir, name) to Uri.parse("content://${context.packageName}.photos/$name")
        }
    }

    /** The photo `uri` is, if it's one of these (a plain name, in the folder). */
    private fun file(uri: Uri): File? {
        val name = uri.lastPathSegment?.takeIf { uri.pathSegments.size == 1 && it.matches(Regex("photo-[0-9]+\\.jpg")) } ?: return null
        return File(folder(context ?: return null), name)
    }

    override fun onCreate() = true

    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor? {
        val file = file(uri) ?: return null
        val access = if (mode.contains('w')) {
            ParcelFileDescriptor.MODE_READ_WRITE or ParcelFileDescriptor.MODE_CREATE or ParcelFileDescriptor.MODE_TRUNCATE
        } else {
            ParcelFileDescriptor.MODE_READ_ONLY
        }
        return ParcelFileDescriptor.open(file, access)
    }

    override fun getType(uri: Uri) = file(uri)?.let { "image/jpeg" }

    // Its name and size, which the page shows for an attachment.
    override fun query(uri: Uri, projection: Array<String>?, selection: String?, args: Array<String>?, order: String?): Cursor? {
        val file = file(uri) ?: return null
        return MatrixCursor(arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE)).apply { addRow(arrayOf(file.name, file.length())) }
    }

    override fun insert(uri: Uri, values: ContentValues?): Uri? = null

    override fun delete(uri: Uri, selection: String?, args: Array<String>?) = 0

    override fun update(uri: Uri, values: ContentValues?, selection: String?, args: Array<String>?) = 0
}
