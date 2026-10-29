//! The Android app's way in: it loads rettui as a library (`librettui.so`)
//! and calls these through `org.zevaryx.rettui.Native`, in its own process.
//! Run there, rather than as a program of its own, rettui can reach Java:
//! rsReticulum finds AutoInterface's network interfaces, USB RNodes and (with
//! btleplug, whose Java classes the app carries) Bluetooth RNodes through it.

use std::ffi::c_void;
use std::io::Write as _;
use std::sync::Mutex;

use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{JNI_VERSION_1_6, jboolean, jint, jobjectArray};

/// Standard output and error are the process's: one run at a time sends
/// them to its file.
static OUTPUT: Mutex<()> = Mutex::new(());

/// Java loading the library: rsReticulum keeps the VM for its JNI calls,
/// and btleplug finds its Java classes (only on this thread, whose class
/// loader is the app's).
#[unsafe(no_mangle)]
pub extern "system" fn JNI_OnLoad(vm: *mut jni::sys::JavaVM, _reserved: *mut c_void) -> jint {
    // SAFETY: the VM Java passes, alive as long as the process.
    if let Ok(vm) = unsafe { jni::JavaVM::from_raw(vm) } {
        rns_interface::android_usb::init_vm(vm);
    }
    // SAFETY: the same VM.
    let vm22 = unsafe { jni22::JavaVM::from_raw(vm.cast()) };
    let bluetooth = vm22.attach_current_thread(|env| Ok::<_, jni22::errors::Error>(btleplug::platform::init(env)));
    // Without it (no Bluetooth here), rsReticulum says so when a Bluetooth
    // RNode is to start.
    if let Ok(Ok(())) = bluetooth {
        rns_interface::ble_rnode::mark_btleplug_initialized();
    }
    JNI_VERSION_1_6
}

/// `Native.run(args, output)`: rettui with `args` (without the program's
/// name), what it prints going to the file `output`. 0 when it worked.
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_zevaryx_rettui_Native_run(env: JNIEnv, _class: JClass, args: jobjectArray, output: JString) -> jint {
    let Ok(output) = env.get_string(output).map(String::from) else { return 2 };
    let mut list = vec!["rettui".to_string()];
    let count = env.get_array_length(args).unwrap_or(0);
    for i in 0..count {
        let Ok(arg) = env.get_object_array_element(args, i) else { return 2 };
        let Ok(arg) = env.get_string(arg.into()) else { return 2 };
        list.push(arg.into());
    }
    let _one = OUTPUT.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&output) else { return 2 };
    let restore = redirect(&file);
    let code = match std::panic::catch_unwind(|| crate::run_with(list)) {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => {
            eprintln!("Error: {e:?}");
            1
        }
        Err(_) => 101,
    };
    let _ = std::io::stdout().flush();
    restore();
    code
}

/// `Native.stop()`: the web UI saves everything and stops, and `run`
/// returns.
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_zevaryx_rettui_Native_stop(_env: JNIEnv, _class: JClass) {
    crate::request_stop();
}

/// `Native.wants()`: the locks rettui needs the app to hold (see
/// [`crate::power`]): bit 1 a wake lock, bit 2 Wi-Fi's multicast lock.
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_zevaryx_rettui_Native_wants(_env: JNIEnv, _class: JClass) -> jint {
    jint::from(crate::power::wants())
}

/// `Native.networkChanged(mobile)`: the phone's own network changed, and
/// `mobile` says whether it's mobile data. When it isn't, connections still
/// on mobile data are shut down to reconnect over it (see
/// [`crate::mobile_data`]). How many were.
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_zevaryx_rettui_Native_networkChanged(_env: JNIEnv, _class: JClass, mobile: jboolean) -> jint {
    if mobile != 0 {
        return 0;
    }
    jint::try_from(crate::mobile_data::leave()).unwrap_or(jint::MAX)
}

/// Send standard output and error to `file` until what's returned is called.
fn redirect(file: &std::fs::File) -> impl FnOnce() {
    use std::os::fd::AsRawFd;
    // SAFETY: plain descriptor calls; the copies are closed once put back.
    let saved = unsafe { [libc::dup(1), libc::dup(2)] };
    unsafe {
        libc::dup2(file.as_raw_fd(), 1);
        libc::dup2(file.as_raw_fd(), 2);
    }
    move || {
        for (fd, copy) in [1, 2].into_iter().zip(saved) {
            if copy >= 0 {
                unsafe {
                    libc::dup2(copy, fd);
                    libc::close(copy);
                }
            }
        }
    }
}
