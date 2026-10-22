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

Some phones stop background apps to save battery. If messages only arrive
while the app is open, allow it to run in the background: *Settings >
Apps > rettui > Battery > Unrestricted* (the wording differs between
phones).

## What it can't do yet

- **Interfaces:** TCP, UDP and AutoInterface (other Reticulum devices on
  the same Wi-Fi) work. RNodes over USB or Bluetooth don't yet.
- **The identity stays in the app.** The web UI never reads or exports
  identity keys (see [Web UI](Web-UI)), and the app has no terminal, so
  there's no way yet to back up its identity or bring one in.
  Uninstalling the app loses it.
- **No terminal UI.**
- It hasn't been tested on many phones yet. Reports of what does and
  doesn't work are welcome in the
  [issues](https://github.com/zevaryx/rettui/issues).

## How it works

The APK carries rettui built for Android, as `lib/<abi>/librettui.so`.
Android unpacks it where the app may run programs from. A foreground
service runs it as

```sh
librettui.so --data-dir <app files>/rettui --rns-config <app files>/rettui/reticulum --web 127.0.0.1:8740
```

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

`build-rust.sh` cross-builds rettui with the NDK's compilers and puts it in
`app/src/main/jniLibs`. Android Studio can open `android/` as a project
once that's done.
