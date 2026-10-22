package org.zevaryx.rettui

import android.app.Activity
import android.app.AlertDialog
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.graphics.Color
import android.hardware.usb.UsbDevice
import android.hardware.usb.UsbManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.text.InputType
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import org.json.JSONObject

/**
 * RNodes plugged in over USB: Android asks before an app may use a USB
 * device, so the permission is asked for here, and the RNode is added to
 * the Reticulum config through the web UI's API (as an RNode interface on
 * an `androidusb://` port, which rsReticulum opens through Android's USB
 * manager). Plugging one in opens this too.
 */
class UsbActivity : Activity() {
    companion object {
        private const val ACTION_PERMISSION = "org.zevaryx.rettui.USB_PERMISSION"

        /** The USB serial chips RNodes use (the same list as rsReticulum's). */
        private val CHIPS = mapOf(
            0x0403 to "FTDI",
            0x10C4 to "Silicon Labs CP210x",
            0x1A86 to "WCH CH340/CH341",
            0x0525 to "CDC-ACM",
            0x2E8A to "Raspberry Pi RP2040",
            0x303A to "Espressif ESP32-S3",
            0x239A to "Adafruit",
            0x1915 to "Nordic nRF52840",
        )
    }

    private val main = Handler(Looper.getMainLooper())
    private lateinit var usb: UsbManager
    private lateinit var list: LinearLayout
    private var asked: UsbDevice? = null

    private val permission = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            val device = asked ?: return
            asked = null
            if (usb.hasPermission(device)) add(device) else say(getString(R.string.usb_denied))
            show()
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        usb = getSystemService(UsbManager::class.java)
        list = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(48, 48, 48, 48)
        }
        setContentView(ScrollView(this).apply {
            setBackgroundColor(Color.rgb(0x16, 0x18, 0x1d))
            addView(list)
        })
        // The answer comes from this app's own pending intent; nothing else
        // may send it. (Before Android 13 the flag is ignored.)
        registerReceiver(permission, IntentFilter(ACTION_PERMISSION), RECEIVER_NOT_EXPORTED)
    }

    override fun onResume() {
        super.onResume()
        show()
    }

    override fun onDestroy() {
        unregisterReceiver(permission)
        super.onDestroy()
    }

    private fun show() {
        list.removeAllViews()
        list.addView(TextView(this).apply {
            text = getString(R.string.usb_note)
            textSize = 14f
            setTextColor(Color.LTGRAY)
            setPadding(0, 0, 0, 24)
        })
        val devices = usb.deviceList.values.sortedBy { it.deviceName }
        if (devices.isEmpty()) {
            list.addView(TextView(this).apply {
                text = getString(R.string.usb_none)
                textSize = 16f
                setTextColor(Color.WHITE)
            })
        }
        for (device in devices) {
            val chip = CHIPS[device.vendorId] ?: "USB device"
            val named = listOfNotNull(device.manufacturerName, device.productName).joinToString(" ").ifBlank { chip }
            list.addView(Button(this).apply {
                isAllCaps = false
                text = "$named\n$chip · %04x:%04x · ${device.deviceName}".format(device.vendorId, device.productId)
                setOnClickListener { choose(device) }
            })
        }
    }

    private fun choose(device: UsbDevice) {
        if (usb.hasPermission(device)) return add(device)
        asked = device
        // Android fills in the answer, so the intent stays mutable.
        val flags = if (Build.VERSION.SDK_INT >= 31) PendingIntent.FLAG_MUTABLE else 0
        val answer = PendingIntent.getBroadcast(this, 0, Intent(ACTION_PERMISSION).setPackage(packageName), flags)
        usb.requestPermission(device, answer)
    }

    /** Ask its name, then add it to the Reticulum config. */
    private fun add(device: UsbDevice) {
        val input = EditText(this).apply {
            setText(getString(R.string.usb_default_name))
            inputType = InputType.TYPE_CLASS_TEXT
        }
        AlertDialog.Builder(this)
            .setTitle(R.string.usb_add)
            .setMessage(R.string.usb_add_note)
            .setView(input)
            .setPositiveButton(R.string.usb_add_button) { _, _ ->
                val name = input.text.toString().trim().ifEmpty { getString(R.string.usb_default_name) }
                Thread { main.post { say(addInterface(name, device)) } }.start()
            }
            .setNegativeButton(android.R.string.cancel, null)
            .show()
    }

    /** Through the web UI's API: what came of it. */
    private fun addInterface(name: String, device: UsbDevice): String {
        val added = Rettui.api(this, "reticulum/interfaces", JSONObject().put("action", "add").put("name", name).put("type", "RNodeInterface"))
        if (!added.ok) return added.text
        // The chip's IDs (and serial number) find it again when it's
        // plugged in elsewhere and gets another name.
        val values = JSONObject()
            .put("port", "androidusb://${device.deviceName}")
            .put("usb_vendor_id", device.vendorId.toString())
            .put("usb_product_id", device.productId.toString())
        runCatching { device.serialNumber }.getOrNull()?.takeIf { it.isNotBlank() }?.let { values.put("usb_serial_number", it) }
        val set = Rettui.api(this, "reticulum/options", JSONObject().put("section", "interface:$name").put("values", values))
        return if (set.ok) getString(R.string.usb_added, name) else set.text
    }

    private fun say(text: String) {
        AlertDialog.Builder(this).setMessage(text).setPositiveButton(android.R.string.ok, null).show()
    }
}
