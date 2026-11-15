#!/usr/bin/env bash
# Build the native library the Android app loads.
#
#   scripts/package-android.sh
#
# Needs the Android NDK and Rust targets aarch64-linux-android and
# x86_64-linux-android. The libraries land in android/app/src/main/jniLibs,
# where Gradle packages them. Set ANDROID_NDK_HOME if the NDK is not inside
# the Android SDK.

set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

die() {
    printf 'package-android: %s\n' "$1" >&2
    exit 1
}

find_ndk() {
    if [[ -n "${ANDROID_NDK_HOME:-}" && -d "$ANDROID_NDK_HOME" ]]; then
        printf '%s\n' "$ANDROID_NDK_HOME"
        return
    fi
    if [[ -n "${ANDROID_NDK_ROOT:-}" && -d "$ANDROID_NDK_ROOT" ]]; then
        printf '%s\n' "$ANDROID_NDK_ROOT"
        return
    fi
    local sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
    if [[ -z "$sdk" && -d "$HOME/Library/Android/sdk" ]]; then
        sdk="$HOME/Library/Android/sdk"
    fi
    if [[ -n "$sdk" && -d "$sdk/ndk" ]]; then
        find "$sdk/ndk" -mindepth 1 -maxdepth 1 -type d | sort | tail -1
        return
    fi
    return 1
}

ndk="$(find_ndk || true)"
[[ -n "$ndk" && -d "$ndk" ]] || die "Android NDK not found. Install it from the SDK manager and set ANDROID_NDK_HOME."

prebuilt=""
for host in darwin-arm64 darwin-x86_64 linux-x86_64 linux-aarch64; do
    if [[ -x "$ndk/toolchains/llvm/prebuilt/$host/bin/clang" ]]; then
        prebuilt="$ndk/toolchains/llvm/prebuilt/$host"
        break
    fi
done
[[ -n "$prebuilt" ]] || die "no LLVM toolchain under $ndk"

api=26
jni="$root/android/app/src/main/jniLibs"

build_one() {
    local triple="$1"
    local abi="$2"
    local prefix="$3"
    local clang="$prebuilt/bin/${prefix}${api}-clang"
    local ar="$prebuilt/bin/llvm-ar"
    [[ -x "$clang" ]] || die "missing $clang"
    if ! rustup target list --installed | grep -qx "$triple"; then
        rustup target add "$triple"
    fi
    local upper
    upper="$(printf '%s' "$triple" | tr '[:lower:]-' '[:upper:]_')"
    echo "Building $triple…"
    env \
        "CC_${triple//-/_}=$clang" \
        "AR_${triple//-/_}=$ar" \
        "CARGO_TARGET_${upper}_LINKER=$clang" \
        "CARGO_TARGET_${upper}_RUSTFLAGS=-C link-arg=-Wl,-z,max-page-size=16384" \
        cargo build -p notia-android --release --target "$triple"
    mkdir -p "$jni/$abi"
    local target_dir="${CARGO_TARGET_DIR:-$root/target}"
    cp "$target_dir/$triple/release/libnotia_android.so" "$jni/$abi/libnotia_android.so"
}

build_one aarch64-linux-android arm64-v8a aarch64-linux-android
build_one x86_64-linux-android x86_64 x86_64-linux-android
printf 'Android libraries are in %s\n' "$jni"
