#!/bin/sh

set -eu

normalize_os() {
  case "$1" in
    linux|Linux) printf '%s\n' "linux" ;;
    windows|Windows|mingw*|msys*|cygwin*) printf '%s\n' "windows" ;;
    *)
      printf 'unsupported sing-box os: %s\n' "$1" >&2
      exit 1
      ;;
  esac
}

normalize_arch() {
  case "$1" in
    amd64|x86_64) printf '%s\n' "amd64" ;;
    386|i386|i686) printf '%s\n' "386" ;;
    arm64|aarch64) printf '%s\n' "arm64" ;;
    armv7|armv7l|arm/v7) printf '%s\n' "armv7" ;;
    armv6|armv6l|arm/v6) printf '%s\n' "armv6" ;;
    armv5|armv5tel|arm/v5) printf '%s\n' "armv5" ;;
    s390x) printf '%s\n' "s390x" ;;
    *)
      printf 'unsupported sing-box arch: %s\n' "$1" >&2
      exit 1
      ;;
  esac
}

OS_INPUT="${1:-$(uname -s)}"
ARCH_INPUT="${2:-$(uname -m)}"
OUTPUT_DIR="${3:-.}"
VERSION="${4:-${SING_BOX_VERSION:-1.13.5}}"

OS="$(normalize_os "$OS_INPUT")"
ARCH="$(normalize_arch "$ARCH_INPUT")"

case "$OS" in
  linux)
    case "$ARCH" in
      386|amd64|arm64|armv7)
        ASSET_CANDIDATES="sing-box-${VERSION}-${OS}-${ARCH}-musl.tar.gz sing-box-${VERSION}-${OS}-${ARCH}.tar.gz"
        ;;
      *)
        ASSET_CANDIDATES="sing-box-${VERSION}-${OS}-${ARCH}.tar.gz"
        ;;
    esac
    ;;
  windows) ASSET_CANDIDATES="sing-box-${VERSION}-${OS}-${ARCH}.zip" ;;
esac

TMP_DIR="$(mktemp -d)"

cleanup() {
  rm -rf "$TMP_DIR"
}

trap cleanup EXIT INT TERM

mkdir -p "$OUTPUT_DIR"

download_asset() {
  asset="$1"
  url="https://github.com/SagerNet/sing-box/releases/download/v${VERSION}/${asset}"

  rm -f "$TMP_DIR/$asset"

  if command -v wget >/dev/null 2>&1; then
    wget -q -O "$TMP_DIR/$asset" "$url"
  else
    curl -fsSL -H 'User-Agent: codex' -o "$TMP_DIR/$asset" "$url"
  fi
}

ASSET=""
for candidate in $ASSET_CANDIDATES; do
  if download_asset "$candidate"; then
    ASSET="$candidate"
    break
  fi
done

if [ -z "$ASSET" ]; then
  printf 'failed to download sing-box asset for %s/%s (tried: %s)\n' "$OS" "$ARCH" "$ASSET_CANDIDATES" >&2
  exit 1
fi

case "$OS" in
  linux)
    tar -xzf "$TMP_DIR/$ASSET" -C "$TMP_DIR"
    EXTRACT_DIR="$TMP_DIR/${ASSET%.tar.gz}"
    cp "$EXTRACT_DIR/sing-box" "$OUTPUT_DIR/sing-box"
    if [ -f "$EXTRACT_DIR/libcronet.so" ]; then
      cp "$EXTRACT_DIR/libcronet.so" "$OUTPUT_DIR/libcronet.so"
    fi
    ;;
  windows)
    if ! command -v unzip >/dev/null 2>&1; then
      printf 'unzip is required to extract Windows sing-box assets\n' >&2
      exit 1
    fi
    unzip -q "$TMP_DIR/$ASSET" -d "$TMP_DIR"
    EXTRACT_DIR="$TMP_DIR/${ASSET%.zip}"
    cp "$EXTRACT_DIR/sing-box.exe" "$OUTPUT_DIR/sing-box.exe"
    if [ -f "$EXTRACT_DIR/libcronet.dll" ]; then
      cp "$EXTRACT_DIR/libcronet.dll" "$OUTPUT_DIR/libcronet.dll"
    fi
    ;;
esac
