#!/usr/bin/env bash
# Build a Notia release image.
#
#   scripts/package.sh              image for this operating system
#   scripts/package.sh macos        Notia.app inside a .dmg
#   scripts/package.sh linux        .tar.gz of the binary (AppImage too, when appimagetool is installed)
#   scripts/package.sh windows      .zip of notia.exe
#   scripts/package.sh all          every image this machine can produce
#
# A macOS, Windows, or Linux GUI build links that system's libraries, so the
# matching image is produced on that system. From macOS or Windows, the Linux
# image is built in Docker when Docker is running. The Windows image is built
# by running this script on Windows.
#
# Images are written to dist/.

set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
dist="$root/dist"

die() {
    printf 'package: %s\n' "$1" >&2
    exit 1
}

host_os() {
    case "$(uname -s)" in
        Darwin) printf 'macos\n' ;;
        Linux) printf 'linux\n' ;;
        MINGW* | MSYS* | CYGWIN*) printf 'windows\n' ;;
        *) die "unsupported operating system: $(uname -s)" ;;
    esac
}

host_arch() {
    case "$(uname -m)" in
        arm64 | aarch64) printf 'aarch64\n' ;;
        x86_64 | amd64) printf 'x86_64\n' ;;
        *) uname -m ;;
    esac
}

require_host() {
    local want="$1"
    local have
    have="$(host_os)"
    if [[ "$have" != "$want" ]]; then
        die "the $want image is built on $want (this machine is $have)"
    fi
}

build_release() {
    echo "Building the release binary…"
    cargo build --release -p notia
}

# --- macOS -----------------------------------------------------------------

package_macos() {
    require_host macos
    local arch stage plist dmg icon
    arch="$(host_arch)"
    build_release

    stage="$dist/Notia.app"
    rm -rf "$stage"
    mkdir -p "$stage/Contents/MacOS" "$stage/Contents/Resources"
    cp "target/release/notia" "$stage/Contents/MacOS/notia"
    chmod +x "$stage/Contents/MacOS/notia"
    icon="$root/crates/notia/resources/Notia.icns"
    [[ -f "$icon" ]] || die "missing $icon"
    cp "$icon" "$stage/Contents/Resources/Notia.icns"

    plist="$stage/Contents/Info.plist"
    cat >"$plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key>
	<string>Notia</string>
	<key>CFBundleDisplayName</key>
	<string>Notia</string>
	<key>CFBundleIdentifier</key>
	<string>app.notia.desktop</string>
	<key>CFBundleVersion</key>
	<string>${version}</string>
	<key>CFBundleShortVersionString</key>
	<string>${version}</string>
	<key>CFBundleExecutable</key>
	<string>notia</string>
	<key>CFBundleIconFile</key>
	<string>Notia.icns</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>LSMinimumSystemVersion</key>
	<string>12.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
</dict>
</plist>
EOF

    codesign --force --sign - "$stage"

    dmg="$dist/Notia-${version}-macos-${arch}.dmg"
    local layout
    layout="$(mktemp -d)"
    cp -R "$stage" "$layout/Notia.app"
    ln -s /Applications "$layout/Applications"
    rm -f "$dmg"
    hdiutil create -volname Notia -srcfolder "$layout" -ov -format UDZO "$dmg" >/dev/null
    rm -rf "$layout"
    echo "Wrote $dmg"
}

# --- Linux -----------------------------------------------------------------

linux_deps='pkg-config libwayland-dev libxkbcommon-dev libx11-dev libxcb1-dev libvulkan-dev libssl-dev cmake g++ libfontconfig1-dev'

package_linux_native() {
    require_host linux
    local arch stage archive
    arch="$(host_arch)"
    build_release

    stage="$dist/Notia-${version}-linux-${arch}"
    rm -rf "$stage"
    mkdir -p "$stage"
    cp "target/release/notia" "$stage/notia"
    chmod +x "$stage/notia"
    archive="${stage}.tar.gz"
    tar -C "$dist" -czf "$archive" "$(basename "$stage")"
    echo "Wrote $archive"
    package_appimage "$stage" "$arch"
}

package_appimage() {
    local stage="$1" arch="$2" appdir icon image
    if ! command -v appimagetool >/dev/null 2>&1; then
        echo "appimagetool is not installed, so no AppImage was made."
        return 0
    fi
    appdir="$dist/Notia.AppDir"
    rm -rf "$appdir"
    mkdir -p "$appdir/usr/bin"
    cp "$stage/notia" "$appdir/usr/bin/notia"
    cat >"$appdir/notia.desktop" <<'EOF'
[Desktop Entry]
Name=Notia
Exec=notia
Icon=notia
Type=Application
Categories=Office;
EOF
    icon="$root/crates/notia/resources/app-icon.png"
    [[ -f "$icon" ]] || die "missing $icon"
    cp "$icon" "$appdir/notia.png"
    mkdir -p "$appdir/usr/share/icons/hicolor/512x512/apps" \
        "$appdir/usr/share/icons/hicolor/1024x1024/apps" \
        "$appdir/usr/share/icons/hicolor/scalable/apps"
    cp "$icon" "$appdir/usr/share/icons/hicolor/512x512/apps/notia.png"
    cp "$root/crates/notia/resources/app-icon@2x.png" \
        "$appdir/usr/share/icons/hicolor/1024x1024/apps/notia.png"
    cp "$root/crates/notia/resources/app-icon.svg" \
        "$appdir/usr/share/icons/hicolor/scalable/apps/notia.svg"
    ln -sf usr/bin/notia "$appdir/AppRun"
    image="$dist/Notia-${version}-linux-${arch}.AppImage"
    ARCH="$arch" appimagetool "$appdir" "$image"
    echo "Wrote $image"
}

package_linux_docker() {
    command -v docker >/dev/null 2>&1 || die "Docker is required to build the Linux image from $(host_os)"
    docker info >/dev/null 2>&1 || die "Docker is not running"
    local arch platform
    arch="$(host_arch)"
    case "$arch" in
        aarch64) platform="linux/arm64" ;;
        x86_64) platform="linux/amd64" ;;
        *) die "no Docker platform for $arch" ;;
    esac
    echo "Building the Linux image in Docker ($platform)…"
    mkdir -p "$dist"
    docker run --rm \
        --platform "$platform" \
        -v "$root":/src:ro \
        -v "$dist":/out \
        -w /src \
        rust:1-bookworm \
        bash -lc "set -euo pipefail
            export DEBIAN_FRONTEND=noninteractive
            apt-get update
            apt-get install -y --no-install-recommends $linux_deps
            cargo build --release -p notia --target-dir /tmp/notia-target
            cp /tmp/notia-target/release/notia /out/notia-linux"
    local stage="$dist/Notia-${version}-linux-${arch}"
    rm -rf "$stage"
    mkdir -p "$stage"
    mv "$dist/notia-linux" "$stage/notia"
    chmod +x "$stage/notia"
    local archive="${stage}.tar.gz"
    tar -C "$dist" -czf "$archive" "$(basename "$stage")"
    echo "Wrote $archive"
}

package_linux() {
    if [[ "$(host_os)" == "linux" ]]; then
        package_linux_native
    else
        package_linux_docker
    fi
}

# --- Windows ---------------------------------------------------------------

package_windows() {
    require_host windows
    local stage archive binary
    build_release
    binary="target/release/notia.exe"
    [[ -f "$binary" ]] || die "missing $binary"
    stage="$dist/Notia-${version}-windows-x86_64"
    rm -rf "$stage"
    mkdir -p "$stage"
    cp "$binary" "$stage/notia.exe"
    archive="${stage}.zip"
    rm -f "$archive"
    if command -v zip >/dev/null 2>&1; then
        (cd "$dist" && zip -r "$(basename "$archive")" "$(basename "$stage")")
    elif command -v powershell.exe >/dev/null 2>&1; then
        powershell.exe -NoProfile -Command "Compress-Archive -Path '$stage' -DestinationPath '$archive' -Force"
    else
        die "install zip or PowerShell to pack the Windows image"
    fi
    echo "Wrote $archive"
}

# --- entry -----------------------------------------------------------------

usage() {
    sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'
}

mkdir -p "$dist"
target="${1:-$(host_os)}"
case "$target" in
    -h | --help | help) usage ;;
    macos) package_macos ;;
    linux) package_linux ;;
    windows) package_windows ;;
    all)
        case "$(host_os)" in
            macos)
                package_macos
                if docker info >/dev/null 2>&1; then
                    package_linux
                else
                    echo "Skipping the Linux image: Docker is not running."
                fi
                echo "The Windows image is built by running scripts/package.sh windows on Windows."
                ;;
            linux) package_linux ;;
            windows)
                package_windows
                if docker info >/dev/null 2>&1; then
                    package_linux
                else
                    echo "Skipping the Linux image: Docker is not running."
                fi
                echo "The macOS image is built by running scripts/package.sh macos on macOS."
                ;;
        esac
        ;;
    *)
        usage >&2
        exit 1
        ;;
esac
