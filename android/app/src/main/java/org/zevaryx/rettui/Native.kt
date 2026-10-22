package org.zevaryx.rettui

/** rettui itself, loaded from the APK as librettui.so (see rettui's src/android.rs). */
object Native {
    init {
        System.loadLibrary("rettui")
    }

    /**
     * Run rettui with `args`, as `rettui <args>` would on a computer, what it
     * prints going to the file `output`. Until it's done: for the web UI,
     * until [stop]. 0 when it worked.
     */
    @JvmStatic
    external fun run(args: Array<String>, output: String): Int

    /** Ask the web UI to save everything and stop. */
    @JvmStatic
    external fun stop()

    /** The locks rettui needs held (see rettui's src/power.rs): 1 a wake lock, 2 Wi-Fi's multicast lock. */
    @JvmStatic
    external fun wants(): Int
}
