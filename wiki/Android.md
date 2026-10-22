An Android app is being built: rettui itself, with its whole Reticulum
stack, running on the phone, and its [web UI](Web-UI) as the app's screen.
It's an early test version. There's no release of it yet, only test builds
from GitHub Actions.

## Getting a test build

1. On GitHub, go to *Actions > Android > Run workflow*. Pick the phones it's
   for (`arm64-v8a` covers almost every phone; `x86_64` is for the
   emulator). Every push to the `feature/android` branch builds one too.
2. When the run is done, download the APK from its *Artifacts*. It comes
   zipped: unzip it to get `rettui-<version>-<commit>-arm64-v8a.apk`.
3. Copy it to the phone and open it. Android asks once to allow installing
   apps from the app you opened it with (Files, a browser).

Unless the repository has a signing key (below), each build is signed with
a different key. Android won't install one over another then: uninstall the
app first, which **deletes its identity and messages**.

### One key for every build

With a signing key in the repository's secrets, each build installs over
the last and keeps everything. Make a key once:

```sh
keytool -genkeypair -keystore rettui.keystore -alias androiddebugkey \
  -keyalg RSA -keysize 2048 -validity 10000 \
  -storepass android -keypass android -dname "CN=rettui test builds"
base64 -w0 rettui.keystore   # on macOS: base64 -i rettui.keystore
```

Then, in *Settings > Secrets and variables > Actions*, add the output as
`ANDROID_KEYSTORE`. If you chose other passwords or another alias, add them
as `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_PASSWORD` and
`ANDROID_KEY_ALIAS`. Keep `rettui.keystore` somewhere safe and out of the
repository.

## Using it

- Opening the app starts rettui, then shows the web UI. Everything else
  is the same as in a browser.
- rettui keeps running when the app is closed, so messages keep
  arriving. Its notification ("Reticulum is running") says so; **Stop**
  in it stops rettui.
- Messages and mentions show as Android notifications while the app isn't
  on screen. Tapping one opens what it's about.
- Recording voice messages and sharing your location ask for the
  microphone or location the first time.
- Downloads (exports, backups, files from messages) go in Downloads.
- If rettui stops, the app shows what it printed, with **Start again**.

## Identity and backups

The web UI never reads or saves identity keys, since anyone with its login
link could then copy yours. The app has a screen of its own for them,
*Identity and backups*. Open it from **🔑 Identity and backups** in the web
UI's Status section, by long-pressing the app's icon, or from the screen
shown when rettui stops. On it:

- **Save a copy of the identity in use:** pick where (Downloads, a USB
  stick, a cloud folder). Keep the copy private: whoever has it can pose as
  you and read what's sent to you.
- **Keep another identity from a file:** an identity file from Sideband,
  NomadNet, MeshChat or another rettui, kept beside yours under a name you
  give it. Switch to it in Status > **👤 Identities**.
- **Save a full backup:** settings, contacts, messages and the identity in
  use, in one file. `rettui restore` restores it on a computer.
- **Restore a backup:** one saved here or by rettui on a computer. rettui
  stops, the backup takes the place of what's here (moved aside into a
  `before-restore-<time>` folder, not deleted), and rettui starts again.

Back up the identity before uninstalling the app: uninstalling deletes
everything the app keeps.

## RNodes over USB

Plug an [RNode](RNode-Radios) into the phone (a USB-C cable, or an OTG
adapter). Android asks whether to open rettui for it: say yes, and rettui
may use it from then on. Otherwise, open **🔌 USB RNode** in the web UI's
Reticulum section, or long-press the app's icon.

The *USB RNodes* screen lists what's plugged in. Tap the RNode, allow rettui
to use it if asked, and give it a name. It's added to the Reticulum config
as an RNode interface on an `androidusb://` port. Its USB IDs (and serial
number) are added too, so it's found again after it's unplugged.

Before restarting Reticulum, set the interface's frequency, bandwidth,
spreading factor, coding rate and TX power in the Reticulum section. Use
what people near you use, and what your country allows. A new interface
starts from an example, which may not be right where you are.

Some phones stop background apps to save battery. If messages only arrive
while the app is open, allow it to run in the background: *Settings >
Apps > rettui > Battery > Unrestricted* (the wording differs between
phones).

## What it can't do yet

- **RNodes over Bluetooth** don't work yet.
- **No terminal UI.**
- It hasn't been tested on many phones yet. Reports of what does and
  doesn't work are welcome in the
  [issues](https://github.com/zevaryx/rettui/issues).

## How it works

The APK carries rettui built for Android as a library,
`lib/<abi>/librettui.so`. A foreground service loads it in a process of its
own and runs it as `rettui` would run on a computer:

```sh
rettui --data-dir <app files>/rettui --rns-config <app files>/rettui/reticulum --web 127.0.0.1:8740
```

Since rettui runs inside the app, rsReticulum can call Android through Java.
It does that to find the network interfaces for AutoInterface and to open
USB RNodes. **Stop** asks rettui to save everything and stop, and then its
process ends, so each start begins fresh. The *Identity and backups* screen
calls rettui's own commands (`identity export`, `identity import`, `backup`,
`restore`) the same way.

The app's screen is a WebView of `http://127.0.0.1:8740`, logged in with
the token rettui keeps in its data directory. Only this phone can reach
it. The service also follows the web UI's live events to show rettui's
notifications.

## Building it yourself

You need the Android SDK with NDK 28.2.13676358, JDK 17 or later, and
the Rust target:

```sh
rustup target add aarch64-linux-android
cd android
ANDROID_HOME=~/Android/Sdk ./build-rust.sh           # or: ./build-rust.sh arm64-v8a x86_64
./gradlew assembleDebug                              # app/build/outputs/apk/debug/app-debug.apk
```

`build-rust.sh` cross-builds rettui as a library (`cargo rustc --lib
--crate-type cdylib`) with the NDK's compilers and puts it in
`app/src/main/jniLibs`. Android Studio can open `android/` as a project
once that's done.
