#!/usr/bin/env bash
# Build the Notia iPhone and iPad app.
#
#   scripts/package-ios.sh              iOS Simulator app in dist/Notia.app
#   scripts/package-ios.sh device       device build, signed ad hoc
#
# Needs Xcode and Rust. The simulator app is installed on the booted simulator
# when one is running. A device build still has to be signed with a development
# team before it will launch on a phone.

set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

# The workspace version in Cargo.toml is the single source of truth. The
# committed mobile/ios/Info.plist carries a copy so the Xcode project opens
# with a sane value; the built bundle is stamped from Cargo.toml below.
version="$(awk '
    { sub(/\r$/, "") }
    /^\[/ { in_package = ($0 == "[workspace.package]"); next }
    in_package && /^version[[:space:]]*=/ {
        sub(/^version[[:space:]]*=[[:space:]]*"/, "")
        sub(/"[[:space:]]*$/, "")
        print
        exit
    }
' Cargo.toml)"
if [[ -z "$version" ]]; then
    printf 'package-ios: no [workspace.package] version in Cargo.toml\n' >&2
    exit 1
fi

# The committed plist is only a readable fallback; say so when it drifts.
committed_version="$(sed -n '/<key>CFBundleShortVersionString<\/key>/{n;s/.*<string>\(.*\)<\/string>.*/\1/p;}' \
    mobile/ios/Info.plist)"
if [[ -n "$committed_version" && "$committed_version" != "$version" ]]; then
    printf 'package-ios: mobile/ios/Info.plist says %s, Cargo.toml says %s; the built app uses %s\n' \
        "$committed_version" "$version" "$version" >&2
fi

if [[ -z "${DEVELOPER_DIR:-}" && -d /Applications/Xcode.app/Contents/Developer ]]; then
    if ! xcodebuild -version >/dev/null 2>&1; then
        export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
    fi
fi

kind="${1:-simulator}"
case "$kind" in
    simulator) triple="aarch64-apple-ios-sim" ;;
    device) triple="aarch64-apple-ios" ;;
    *)
        printf 'package-ios: use simulator or device\n' >&2
        exit 1
        ;;
esac

if ! rustup target list --installed | grep -qx "$triple"; then
    rustup target add "$triple"
fi

# Braces: macOS /bin/bash 3.2 treats the ellipsis as part of $triple.
echo "Building Notia for ${triple}…"
IPHONEOS_DEPLOYMENT_TARGET=15.0 \
    cargo build -p notia --release --target "$triple"

target_dir="${CARGO_TARGET_DIR:-$root/target}"
app="$root/dist/Notia.app"
rm -rf "$app"
mkdir -p "$app"
cp "$target_dir/$triple/release/notia" "$app/notia"
cp "$root/mobile/ios/Info.plist" "$app/Info.plist"
# Stamp the built bundle so the app's version always matches the release,
# even when the committed plist has fallen behind.
# PlistBuddy lives in /usr/libexec. Xcode 27's xcrun no longer finds it.
if [[ -x /usr/libexec/PlistBuddy ]]; then
    /usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" "$app/Info.plist"
elif xcrun --find PlistBuddy >/dev/null 2>&1; then
    xcrun PlistBuddy -c "Set :CFBundleShortVersionString $version" "$app/Info.plist"
else
    printf 'package-ios: PlistBuddy is unavailable, keeping the Info.plist version\n' >&2
fi
codesign --force --sign - "$app"
printf 'Built %s\n' "$app"

if [[ "$kind" == simulator ]] && xcrun simctl list devices booted | grep -q Booted; then
    xcrun simctl install booted "$app"
    xcrun simctl launch booted app.notia.ios || true
    echo "Installed on the booted simulator."
fi
