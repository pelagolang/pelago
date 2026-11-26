#!/bin/sh
# Install Pelago on Linux or macOS.
#
#   curl -fsSL https://pelagolang.github.io/install.sh | sh
#   curl -fsSL https://pelagolang.github.io/install.sh | sh -s -- --channel nightly
#
# Options: --channel stable|beta|rc|nightly (default stable)   --version X.Y.Z (an exact release)
#          --dir DIR (default ~/.pelago/bin)                   --allow-unsigned (go on without a signature)
# It checks the SHA-256 of the archive against the release's SHA256SUMS, and the Ed25519 signature of
# SHA256SUMS when the release has one and `openssl` (3.x) is installed. Nothing is installed on a failure.
set -eu

REPO="pelagolang/pelago"
API="${PELAGO_UPDATE_API:-https://api.github.com/repos/$REPO}"
DOWNLOAD="${PELAGO_DOWNLOAD_BASE:-https://github.com/$REPO/releases/download}"
# The release public key (PEM). Filled in when the first signed release is cut; see docs/release/README.md.
PUBKEY=""

channel="stable"; version=""; dir="$HOME/.pelago/bin"; allow_unsigned=0
while [ $# -gt 0 ]; do
  case "$1" in
    --channel) channel="${2:?--channel needs a value}"; shift 2 ;;
    --version) version="${2:?--version needs a value}"; shift 2 ;;
    --dir) dir="${2:?--dir needs a value}"; shift 2 ;;
    --allow-unsigned) allow_unsigned=1; shift ;;
    -h|--help) sed -n '2,10p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done
case "$channel" in stable|beta|rc|nightly) ;; *) echo "unknown channel: $channel" >&2; exit 2 ;; esac

die() { echo "error: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "$1 is required"; }
need curl; need tar

case "$(uname -s)" in
  Linux) os=linux ;;
  Darwin) os=macos ;;
  *) die "unsupported system $(uname -s); on Windows use install.ps1" ;;
esac
case "$(uname -m)" in
  x86_64|amd64) arch=x86_64 ;;
  aarch64|arm64) arch=aarch64 ;;
  *) die "unsupported processor $(uname -m)" ;;
esac
# An x86-64 shell on an Apple silicon Mac still wants the native build.
if [ "$os" = macos ] && [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then arch=aarch64; fi
target="$arch-$os"

fetch() { curl -fsSL --retry 2 -A pelago-install "$@"; }
sort_versions() { sort -V 2>/dev/null || sort; }

releases="$(fetch -H 'Accept: application/vnd.github+json' "$API/releases?per_page=100")" || die "cannot read the release list"
tags="$(printf '%s\n' "$releases" | grep -o '"tag_name": *"[^"]*"' | sed 's/.*"\(v[^"]*\)"$/\1/')"

if [ -n "$version" ]; then
  tag="v${version#v}"
  printf '%s\n' "$tags" | grep -qx "$tag" || die "no release $version"
  name_version="${tag#v}"
elif [ "$channel" = nightly ]; then
  tag=nightly
  rel="$(fetch -H 'Accept: application/vnd.github+json' "$API/releases/tags/nightly")" || die "there is no nightly release yet"
  assets_url="$(printf '%s\n' "$rel" | grep -o '"assets_url": *"[^"]*"' | head -n 1 | sed 's/.*: *"\(.*\)"/\1/')"
  [ -n "$assets_url" ] || die "cannot find the nightly assets"
  # The rolling release holds many builds, so read its asset list a page at a time.
  assets=""
  page=1
  while :; do
    chunk="$(fetch -H 'Accept: application/vnd.github+json' "$assets_url?per_page=100&page=$page")" || die "cannot read the nightly assets"
    assets="$assets
$chunk"
    [ "$(printf '%s\n' "$chunk" | grep -o '"browser_download_url"' | wc -l)" -ge 100 ] || break
    page=$((page + 1))
  done
  name_version="$(printf '%s\n' "$assets" | grep -o "\"name\": *\"pelago-[^\"]*-$target\.tar\.gz\"" | sed "s/.*\"pelago-\(.*\)-$target\.tar\.gz\"/\1/" | sort_versions | tail -n 1)"
  [ -n "$name_version" ] || die "no nightly build for $target yet"
else
  case "$channel" in
    stable) pattern='^v[0-9]+\.[0-9]+\.[0-9]+$' ;;
    beta) pattern='^v[0-9]+\.[0-9]+\.[0-9]+-beta\.[0-9]+$' ;;
    rc) pattern='^v[0-9]+\.[0-9]+\.[0-9]+-rc\.[0-9]+$' ;;
  esac
  tag="$(printf '%s\n' "$tags" | grep -E "$pattern" | sort_versions | tail -n 1)"
  [ -n "$tag" ] || die "no $channel release yet"
  name_version="${tag#v}"
fi

archive="pelago-$name_version-$target.tar.gz"
if [ "$tag" = nightly ]; then sums="SHA256SUMS-$name_version"; else sums="SHA256SUMS"; fi
echo "installing pelago $name_version ($target)"

tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT INT TERM
fetch -o "$tmp/$archive" "$DOWNLOAD/$tag/$archive" || die "cannot download $archive"
fetch -o "$tmp/$sums" "$DOWNLOAD/$tag/$sums" || die "cannot download $sums"

expected="$(grep "  $archive\$" "$tmp/$sums" | cut -d' ' -f1)"
[ -n "$expected" ] || die "$archive is not in $sums"
if command -v sha256sum >/dev/null 2>&1; then actual="$(sha256sum "$tmp/$archive" | cut -d' ' -f1)"
elif command -v shasum >/dev/null 2>&1; then actual="$(shasum -a 256 "$tmp/$archive" | cut -d' ' -f1)"
else die "sha256sum or shasum is required"; fi
[ "$expected" = "$actual" ] || die "checksum mismatch for $archive"
echo "checksum ok"

if fetch -o "$tmp/$sums.sig" "$DOWNLOAD/$tag/$sums.sig" 2>/dev/null; then
  if [ -n "$PUBKEY" ] && command -v openssl >/dev/null 2>&1; then
    printf '%s\n' "$PUBKEY" > "$tmp/key.pub"
    openssl pkeyutl -verify -rawin -pubin -inkey "$tmp/key.pub" -in "$tmp/$sums" -sigfile "$tmp/$sums.sig" >/dev/null 2>&1 \
      || die "the signature on $sums is not valid"
    echo "signature ok"
  elif [ "$allow_unsigned" = 1 ]; then
    echo "signature not checked (no key in this script, or no openssl): going on because of --allow-unsigned"
  else
    die "cannot check the signature here (no release key in this script, or no openssl); pass --allow-unsigned to go on with the checksum only"
  fi
elif [ "$allow_unsigned" = 1 ]; then
  echo "this release has no signature: going on because of --allow-unsigned"
else
  die "this release has no signature; pass --allow-unsigned to go on with the checksum only"
fi

tar -xzf "$tmp/$archive" -C "$tmp"
bin="$tmp/pelago-$name_version-$target/pelago"
[ -x "$bin" ] || die "the archive has no pelago binary"
"$bin" --version | grep -q "^pelago $name_version" || die "the downloaded binary does not report $name_version"

mkdir -p "$dir"
cp "$bin" "$dir/.pelago.new" && chmod 755 "$dir/.pelago.new" && mv -f "$dir/.pelago.new" "$dir/pelago"
echo "$channel" > "$dir/pelago.channel"
echo "installed $dir/pelago"
case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo "add it to your PATH:  export PATH=\"$dir:\$PATH\"" ;;
esac
