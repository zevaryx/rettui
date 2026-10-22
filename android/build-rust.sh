#!/usr/bin/env bash
# Build rettui for Android and put it in the app, as the native library
# app/src/main/jniLibs/<abi>/librettui.so, which the app loads.
#
#   ./build-rust.sh [arm64-v8a|x86_64 ...]    (default: arm64-v8a)
#
# Needs the Android NDK (ANDROID_NDK_HOME, or the version build.gradle.kts
# names under ANDROID_HOME) and the Rust targets (rustup target add
# aarch64-linux-android x86_64-linux-android).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
api=26
ndk_version="$(sed -n 's/.*ndkVersion = "\(.*\)".*/\1/p' "$here/app/build.gradle.kts")"
ndk="${ANDROID_NDK_HOME:-${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}/ndk/$ndk_version}"
case "$(uname -s)" in
  Darwin) host=darwin-x86_64 ;;
  *) host=linux-x86_64 ;;
esac
bin="$ndk/toolchains/llvm/prebuilt/$host/bin"
[ -d "$bin" ] || { echo "No Android NDK at $ndk (set ANDROID_NDK_HOME)" >&2; exit 1; }

[ $# -gt 0 ] || set -- arm64-v8a
for abi in "$@"; do
  case "$abi" in
    arm64-v8a) target=aarch64-linux-android ;;
    x86_64) target=x86_64-linux-android ;;
    *) echo "Unknown ABI $abi (arm64-v8a or x86_64)" >&2; exit 1 ;;
  esac
  env_target="$(echo "$target" | tr 'a-z-' 'A-Z_')"
  export "CARGO_TARGET_${env_target}_LINKER=$bin/$target$api-clang"
  export "CC_${target//-/_}=$bin/$target$api-clang"
  export "AR_${target//-/_}=$bin/llvm-ar"
  cargo rustc --locked --release --lib --crate-type cdylib --target "$target" --manifest-path "$here/../Cargo.toml"
  mkdir -p "$here/app/src/main/jniLibs/$abi"
  "$bin/llvm-strip" -o "$here/app/src/main/jniLibs/$abi/librettui.so" "$here/../target/$target/release/librettui.so"
  echo "Built $here/app/src/main/jniLibs/$abi/librettui.so"
done

# btleplug (Bluetooth RNodes) calls Java classes of its own, which the app
# carries: taken from the btleplug crate Cargo.lock has, so the two match.
btleplug="$(cargo metadata --locked --format-version 1 --manifest-path "$here/../Cargo.toml" --filter-platform aarch64-linux-android |
  python3 -c 'import json, sys; print(next(p["manifest_path"] for p in json.load(sys.stdin)["packages"] if p["name"] == "btleplug"))')"
rm -rf "$here/btleplug/java"
cp -R "$(dirname "$btleplug")/src/droidplug/java/src/main/java" "$here/btleplug/java"
cp "$(dirname "$btleplug")/LICENSE.md" "$here/btleplug/java/LICENSE.md"
echo "Took btleplug's Java classes from $(dirname "$btleplug")"
