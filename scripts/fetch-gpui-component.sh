#!/usr/bin/env bash
# Cargo.toml patches gpui-component from third_party/gpui-component.
# That tree is gitignored, so a GitHub Actions checkout does not contain it.
# Download 0.10.0 and widen context-menu icon gaps from 4px to 8px.
# A tree that is already present is left as-is, so local edits stay put.

set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
dest="$root/third_party/gpui-component"
version="0.10.0"
url="https://static.crates.io/crates/gpui-component/gpui-component-${version}.crate"
checksum="607c90f6985ac2b96e54102338994eaea143aff521375c6f9e7fc7ecb3ba3646"
patch_file="$root/patches/gpui-component-menu-gap.patch"

if [[ -f "$dest/Cargo.toml" ]] && grep -q '^name = "gpui-component"$' "$dest/Cargo.toml"; then
    exit 0
fi

if [[ ! -f "$patch_file" ]]; then
    printf 'fetch-gpui-component: missing %s\n' "$patch_file" >&2
    exit 1
fi

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        printf 'fetch-gpui-component: need sha256sum or shasum\n' >&2
        exit 1
    fi
}

if ! command -v curl >/dev/null 2>&1; then
    printf 'fetch-gpui-component: need curl\n' >&2
    exit 1
fi
if ! command -v patch >/dev/null 2>&1; then
    printf 'fetch-gpui-component: need patch\n' >&2
    exit 1
fi

echo "Fetching gpui-component ${version}…"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
crate="$tmp/gpui-component-${version}.crate"
curl -fsSL --retry 3 -o "$crate" "$url"
sum="$(sha256_file "$crate")"
if [[ "$sum" != "$checksum" ]]; then
    printf 'fetch-gpui-component: checksum mismatch for gpui-component %s\n' "$version" >&2
    printf 'expected %s\n' "$checksum" >&2
    printf 'got      %s\n' "$sum" >&2
    exit 1
fi

tar -xzf "$crate" -C "$tmp"
stage="$tmp/gpui-component-${version}"
if [[ ! -f "$stage/Cargo.toml" ]]; then
    printf 'fetch-gpui-component: crate did not contain Cargo.toml\n' >&2
    exit 1
fi
patch -d "$stage" -p1 -i "$patch_file"

mkdir -p "$root/third_party"
rm -rf "$dest"
mv "$stage" "$dest"
echo "Patched gpui-component context-menu spacing."
