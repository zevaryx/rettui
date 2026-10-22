package org.zevaryx.rettui

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.app.AlertDialog
import android.app.PendingIntent
import android.bluetooth.BluetoothManager
import android.bluetooth.le.ScanCallback
import android.bluetooth.le.ScanFilter
import android.bluetooth.le.ScanResult
import android.bluetooth.le.ScanSettings
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.graphics.Color
import android.graphics.Typeface
import android.hardware.usb.UsbDevice
import android.hardware.usb.UsbManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.ParcelUuid
import android.text.InputType
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import org.json.JSONObject
import java.util.UUID

/**
 * RNodes, over USB or Bluetooth. Android asks before an app may use either,
 * so the permission is asked for here; then the RNode is added to the
 * Reticulum config through the web UI's API, as an RNode interface on an
 * `androidusb://` port (which rsReticulum opens through Android's USB
 * manager) or a `ble://` one (through btleplug). Plugging an RNode in opens
 * this too.
 */
class RNodesActivity : Activity() {
    companion object {
        private const val ACTION_PERMISSION = "org.zevaryx.rettui.USB_PERMISSION"
        private const val ASK_BLUETOOTH = 1
        private const val SCAN_MILLIS = 10_000L

        /** RNodes over Bluetooth offer the Nordic UART service. */
        private val NUS = UUID.fromString("6e400001-b5a3-f393-e0a9-e50e24dcca9e")

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

    /** One heard over Bluetooth: its name (if it gave one) and address. */
    private data class Heard(val name: String?, val address: String)

    private val main = Handler(Looper.getMainLooper())
    private lateinit var usb: UsbManager
    private lateinit var list: LinearLayout
    private var asked: UsbDevice? = null
    private val heard = linkedMapOf<String, Heard>()
    private var scanning = false

    private val permission = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            val device = asked ?: return
            asked = null
            if (usb.hasPermission(device)) addUsb(device) else say(getString(R.string.usb_denied))
            show()
        }
    }

    private val scan = object : ScanCallback() {
        @SuppressLint("MissingPermission")
        override fun onScanResult(callbackType: Int, result: ScanResult) {
            val address = result.device.address ?: return
            val name = result.scanRecord?.deviceName ?: runCatching { result.device.name }.getOrNull()
            if (heard.put(address, Heard(name, address)) == null) main.post { show() }
        }

        override fun onScanFailed(errorCode: Int) {
            scanning = false
            main.post {
                say(getString(R.string.bluetooth_failed, errorCode))
                show()
            }
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

    override fun onPause() {
        stopScan()
        super.onPause()
    }

    override fun onDestroy() {
        unregisterReceiver(permission)
        super.onDestroy()
    }

    private fun show() {
        list.removeAllViews()
        heading(getString(R.string.usb))
        note(getString(R.string.usb_note))
        val devices = usb.deviceList.values.sortedBy { it.deviceName }
        if (devices.isEmpty()) note(getString(R.string.usb_none))
        for (device in devices) {
            val chip = CHIPS[device.vendorId] ?: "USB device"
            val named = listOfNotNull(device.manufacturerName, device.productName).joinToString(" ").ifBlank { chip }
            button(getString(R.string.usb_device, named, chip, "%04x:%04x".format(device.vendorId, device.productId), device.deviceName)) {
                chooseUsb(device)
            }
        }

        heading(getString(R.string.bluetooth))
        note(getString(R.string.bluetooth_note))
        button(getString(if (scanning) R.string.bluetooth_scanning else R.string.bluetooth_find)) { if (!scanning) findBluetooth() }
        for (one in heard.values) {
            button(getString(R.string.bluetooth_device, one.name ?: getString(R.string.bluetooth_unnamed), one.address)) { addBluetooth(one) }
        }
    }

    private fun heading(text: String) = list.addView(TextView(this).apply {
        this.text = text
        textSize = 20f
        setTypeface(typeface, Typeface.BOLD)
        setTextColor(Color.WHITE)
        setPadding(0, 32, 0, 8)
    })

    private fun note(text: String) = list.addView(TextView(this).apply {
        this.text = text
        textSize = 14f
        setTextColor(Color.LTGRAY)
        setPadding(0, 0, 0, 12)
    })

    private fun button(text: String, run: () -> Unit) = list.addView(Button(this).apply {
        this.text = text
        isAllCaps = false
        setOnClickListener { run() }
    })

    // ---- USB ------------------------------------------------------------------

    private fun chooseUsb(device: UsbDevice) {
        if (usb.hasPermission(device)) return addUsb(device)
        asked = device
        // Android fills in the answer, so the intent stays mutable.
        val flags = if (Build.VERSION.SDK_INT >= 31) PendingIntent.FLAG_MUTABLE else 0
        val answer = PendingIntent.getBroadcast(this, 0, Intent(ACTION_PERMISSION).setPackage(packageName), flags)
        usb.requestPermission(device, answer)
    }

    private fun addUsb(device: UsbDevice) = askName(getString(R.string.usb_default_name)) {
        // The chip's IDs (and serial number) find it again when it's
        // plugged in elsewhere and gets another name.
        val values = JSONObject()
            .put("port", "androidusb://${device.deviceName}")
            .put("usb_vendor_id", device.vendorId.toString())
            .put("usb_product_id", device.productId.toString())
        runCatching { device.serialNumber }.getOrNull()?.takeIf { it.isNotBlank() }?.let { values.put("usb_serial_number", it) }
        values
    }

    // ---- Bluetooth --------------------------------------------------------------

    /** What Android asks for before an app finds and connects to Bluetooth devices. */
    private fun bluetoothPermissions(): Array<String> = if (Build.VERSION.SDK_INT >= 31) {
        arrayOf(Manifest.permission.BLUETOOTH_SCAN, Manifest.permission.BLUETOOTH_CONNECT)
    } else {
        arrayOf(Manifest.permission.ACCESS_FINE_LOCATION)
    }

    private fun findBluetooth() {
        val missing = bluetoothPermissions().filter { checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }
        if (missing.isNotEmpty()) return requestPermissions(missing.toTypedArray(), ASK_BLUETOOTH)
        val adapter = getSystemService(BluetoothManager::class.java)?.adapter
        if (adapter == null || !adapter.isEnabled) return say(getString(R.string.bluetooth_off))
        startScan()
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<String>, results: IntArray) {
        if (requestCode != ASK_BLUETOOTH) return
        if (results.isNotEmpty() && results.all { it == PackageManager.PERMISSION_GRANTED }) findBluetooth() else say(getString(R.string.bluetooth_denied))
    }

    @SuppressLint("MissingPermission")
    private fun startScan() {
        val scanner = getSystemService(BluetoothManager::class.java)?.adapter?.bluetoothLeScanner ?: return
        heard.clear()
        scanning = true
        val filters = listOf(ScanFilter.Builder().setServiceUuid(ParcelUuid(NUS)).build())
        scanner.startScan(filters, ScanSettings.Builder().setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY).build(), scan)
        main.postDelayed({ stopScan() }, SCAN_MILLIS)
        show()
    }

    @SuppressLint("MissingPermission")
    private fun stopScan() {
        if (!scanning) return
        scanning = false
        runCatching { getSystemService(BluetoothManager::class.java)?.adapter?.bluetoothLeScanner?.stopScan(scan) }
        show()
    }

    /** By its name, which RNodes keep (`RNode 3B87`), or else its address. */
    private fun addBluetooth(one: Heard) = askName(one.name ?: getString(R.string.bluetooth_default_name)) {
        JSONObject().put("port", "ble://${one.name ?: one.address}")
    }

    // ---- Adding it --------------------------------------------------------------

    /** Ask its name in the config, then add it with the options `values` gives. */
    private fun askName(suggested: String, values: () -> JSONObject) {
        val input = EditText(this).apply {
            setText(suggested)
            inputType = InputType.TYPE_CLASS_TEXT
        }
        AlertDialog.Builder(this)
            .setTitle(R.string.rnode_add)
            .setMessage(R.string.rnode_add_note)
            .setView(input)
            .setPositiveButton(R.string.rnode_add_button) { _, _ ->
                val name = input.text.toString().trim().ifEmpty { suggested }
                val options = values()
                Thread {
                    val said = addInterface(name, options)
                    main.post { say(said) }
                }.start()
            }
            .setNegativeButton(android.R.string.cancel, null)
            .show()
    }

    /** Through the web UI's API: what came of it. */
    private fun addInterface(name: String, values: JSONObject): String {
        val added = Rettui.api(this, "reticulum/interfaces", JSONObject().put("action", "add").put("name", name).put("type", "RNodeInterface"))
        if (!added.ok) return added.text
        val set = Rettui.api(this, "reticulum/options", JSONObject().put("section", "interface:$name").put("values", values))
        return if (set.ok) getString(R.string.rnode_added, name) else set.text
    }

    private fun say(text: String) {
        AlertDialog.Builder(this).setMessage(text).setPositiveButton(android.R.string.ok, null).show()
    }
}
