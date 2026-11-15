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

echo "Building Notia for $triple…"
IPHONEOS_DEPLOYMENT_TARGET=15.0 \
    cargo build -p notia --release --target "$triple"

target_dir="${CARGO_TARGET_DIR:-$root/target}"
app="$root/dist/Notia.app"
rm -rf "$app"
mkdir -p "$app"
cp "$target_dir/$triple/release/notia" "$app/notia"
cp "$root/mobile/ios/Info.plist" "$app/Info.plist"
codesign --force --sign - "$app"
printf 'Built %s\n' "$app"

if [[ "$kind" == simulator ]] && xcrun simctl list devices booted | grep -q Booted; then
    xcrun simctl install booted "$app"
    xcrun simctl launch booted app.notia.ios || true
    echo "Installed on the booted simulator."
fi
