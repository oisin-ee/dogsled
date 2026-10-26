#!/bin/sh
# Herdr [[build]] step: install the dogsled release that matches this plugin's
# manifest version at bin/dogsled. The board pane runs that file, never a
# dogsled from the Herdr server's PATH.
#
# It downloads the cargo-dist archive for this platform and checks its SHA-256.
# When the release or platform has no archive, it builds from source with
# `cargo build --release --locked`. A checksum mismatch stops the install.
# DOGSLED_RELEASES replaces the release download base (a mirror, or file://).
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
out="$root/bin/dogsled"
base="${DOGSLED_RELEASES:-https://github.com/oisin-ee/dogsled/releases/download}"
version=$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' "$root/herdr-plugin.toml" | head -n 1)
say() { printf 'dogsled: %s\n' "$*" >&2; }

install_file() {
  mkdir -p "$root/bin"
  cp "$1" "$out.new"
  chmod 0755 "$out.new"
  mv -f "$out.new" "$out"
  say "installed $out ($2)"
}

build() {
  say "$1; building v$version from source"
  if ! command -v cargo >/dev/null 2>&1; then
    say "cargo is not on PATH. Install Rust (https://rustup.rs) and install the plugin again."
    exit 1
  fi
  if ! command -v dx >/dev/null 2>&1; then
    say "dx is not on PATH. Install it with 'cargo install dioxus-cli --version 0.7.10 --locked' and install the plugin again."
    exit 1
  fi
  (cd "$root/web" && dx bundle --web --release)
  (cd "$root" && cargo build --release --locked --bin dogsled)
  install_file "$root/target/release/dogsled" "built from source"
  exit 0
}

[ -n "$version" ] || build "herdr-plugin.toml has no version"
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64 | Darwin-aarch64) triple=aarch64-apple-darwin ;;
  Linux-x86_64 | Linux-amd64) triple=x86_64-unknown-linux-gnu ;;
  *) build "no release archive for $(uname -s)-$(uname -m)" ;;
esac
command -v curl >/dev/null 2>&1 || build "curl is not on PATH"

archive="dogsled-$triple.tar.xz"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
url="$base/v$version/$archive"
curl -fsSL -o "$tmp/$archive" "$url" || build "could not download $url"
curl -fsSL -o "$tmp/$archive.sha256" "$url.sha256" || build "could not download $url.sha256"

# cargo-dist writes `<sha256> *<archive>`.
expected=$(sed -n "s/^\([0-9a-f]\{64\}\) [ *]$archive\$/\1/p" "$tmp/$archive.sha256")
actual=$( (sha256sum "$tmp/$archive" 2>/dev/null || shasum -a 256 "$tmp/$archive") | cut -d ' ' -f 1)
if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
  say "checksum mismatch for $archive (expected ${expected:-none}, got $actual)"
  exit 1
fi

tar -xJf "$tmp/$archive" -C "$tmp"
install_file "$tmp/dogsled-$triple/dogsled" "release v$version, $triple"
