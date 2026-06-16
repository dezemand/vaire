#!/bin/sh
# Vairë installer for Linux and macOS.
#
# Downloads a prebuilt `vaire` binary from the latest GitHub Release and installs
# it into a bin directory on (or addable to) your PATH.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/dezemand/vaire/main/install.sh | sh
#
# Environment overrides:
#   VAIRE_VERSION      Tag to install (e.g. v0.1.0). Default: latest release.
#   VAIRE_INSTALL_DIR  Where to put the binary. Default: $HOME/.local/bin.

set -eu

REPO="dezemand/vaire"
BIN="vaire"

# --- pretty output -----------------------------------------------------------
if [ -t 1 ]; then
  bold="$(printf '\033[1m')"; dim="$(printf '\033[2m')"
  red="$(printf '\033[31m')"; grn="$(printf '\033[32m')"; reset="$(printf '\033[0m')"
else
  bold=""; dim=""; red=""; grn=""; reset=""
fi
info() { printf '%s\n' "${dim}$*${reset}"; }
ok()   { printf '%s\n' "${grn}$*${reset}"; }
err()  { printf '%s\n' "${red}error:${reset} $*" >&2; }
die()  { err "$@"; exit 1; }

need() { command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"; }

# --- detect platform ---------------------------------------------------------
os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Linux)  os_name="linux" ;;
  Darwin) os_name="darwin" ;;
  *) die "unsupported OS: $os. Build from source with: cargo install --path ." ;;
esac

case "$arch" in
  x86_64|amd64)  arch_name="x86_64" ;;
  arm64|aarch64) arch_name="aarch64" ;;
  *) die "unsupported architecture: $arch" ;;
esac

# Map platform to the release target triple. The release workflow currently
# publishes: x86_64 musl linux, aarch64 macOS, and x86_64 windows.
case "${os_name}-${arch_name}" in
  linux-x86_64)   target="x86_64-unknown-linux-musl" ;;
  darwin-aarch64) target="aarch64-apple-darwin" ;;
  darwin-x86_64)
    die "no prebuilt binary for Intel macOS. Build from source with: cargo install --path ." ;;
  linux-aarch64)
    die "no prebuilt binary for arm64 Linux. Build from source with: cargo install --path ." ;;
  *) die "no prebuilt binary for ${os_name}-${arch_name}. Build from source with: cargo install --path ." ;;
esac

# --- pick a downloader -------------------------------------------------------
if command -v curl >/dev/null 2>&1; then
  dl() { curl -fsSL "$1" -o "$2"; }
  dl_stdout() { curl -fsSL "$1"; }
elif command -v wget >/dev/null 2>&1; then
  dl() { wget -qO "$2" "$1"; }
  dl_stdout() { wget -qO- "$1"; }
else
  die "need curl or wget to download"
fi
need tar

# --- resolve version ---------------------------------------------------------
version="${VAIRE_VERSION:-}"
if [ -z "$version" ]; then
  info "Resolving latest release..."
  # Parse the tag_name from the GitHub releases API without requiring jq.
  version="$(dl_stdout "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep -m1 '"tag_name"' \
    | sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')"
  [ -n "$version" ] || die "could not determine the latest release version. Set VAIRE_VERSION."
fi

stem="${BIN}-${version}-${target}"
asset="${stem}.tar.gz"
url="https://github.com/${REPO}/releases/download/${version}/${asset}"

# --- install dir -------------------------------------------------------------
install_dir="${VAIRE_INSTALL_DIR:-$HOME/.local/bin}"

printf '%s\n' "${bold}Installing ${BIN} ${version}${reset} ${dim}(${target})${reset}"
info "  from $url"
info "  to   $install_dir"

# --- download + extract ------------------------------------------------------
tmp="$(mktemp -d 2>/dev/null || mktemp -d -t vaire)"
trap 'rm -rf "$tmp"' EXIT INT TERM

dl "$url" "$tmp/$asset" || die "download failed: $url"
tar -xzf "$tmp/$asset" -C "$tmp" || die "failed to extract $asset"

# Archive contains a top-level directory ($stem) holding the binary.
src="$tmp/$stem/$BIN"
[ -f "$src" ] || src="$tmp/$BIN"          # fall back to a flat archive
[ -f "$src" ] || die "binary not found in archive"

mkdir -p "$install_dir"
chmod +x "$src"
mv -f "$src" "$install_dir/$BIN"

ok "Installed $install_dir/$BIN"

# --- PATH hint ---------------------------------------------------------------
case ":$PATH:" in
  *":$install_dir:"*) ;;
  *)
    printf '%s\n' "${bold}Note:${reset} $install_dir is not on your PATH."
    printf '%s\n' "Add it by appending this to your shell profile (~/.bashrc, ~/.zshrc, ...):"
    printf '\n  export PATH="%s:$PATH"\n\n' "$install_dir"
    ;;
esac

printf 'Run %s%s --help%s to get started.\n' "$bold" "$BIN" "$reset"
