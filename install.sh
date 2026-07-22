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
#   VAIRE_VERSION      Version to install (e.g. 0.1.0; a leading v is accepted).
#                      Default: the latest release — skipped when the installed
#                      vaire is already at or above it. A pinned version always
#                      installs.
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

# ver_ge A B — true when semver A is at or above B (bare X.Y.Z compared numerically,
# pre-release suffixes ignored; anything unparseable compares false, so installation
# proceeds).
ver_ge() {
  a="${1%%-*}" b="${2%%-*}"
  case "$a" in *.*.*) ;; *) return 1 ;; esac
  case "$b" in *.*.*) ;; *) return 1 ;; esac
  a1="${a%%.*}"; a3="${a##*.}"; a2="${a#*.}"; a2="${a2%%.*}"
  b1="${b%%.*}"; b3="${b##*.}"; b2="${b#*.}"; b2="${b2%%.*}"
  for n in "$a1" "$a2" "$a3" "$b1" "$b2" "$b3"; do
    case "$n" in ''|*[!0-9]*) return 1 ;; esac
  done
  [ "$a1" -ne "$b1" ] && { [ "$a1" -gt "$b1" ]; return; }
  [ "$a2" -ne "$b2" ] && { [ "$a2" -gt "$b2" ]; return; }
  [ "$a3" -ge "$b3" ]
}

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
pinned="$version"
if [ -z "$version" ]; then
  info "Resolving latest release..."
  # Parse the tag_name from the GitHub releases API without requiring jq.
  version="$(dl_stdout "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep -m1 '"tag_name"' \
    | sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')"
  [ -n "$version" ] || die "could not determine the latest release version. Set VAIRE_VERSION."
fi
# Versions are bare (0.2.0); the v prefix exists only on the git tag (and the
# asset/download paths built from it). VAIRE_VERSION is accepted either way.
version="${version#v}"
tag="v${version}"

stem="${BIN}-${tag}-${target}"
asset="${stem}.tar.gz"
url="https://github.com/${REPO}/releases/download/${tag}/${asset}"

# --- install dir -------------------------------------------------------------
install_dir="${VAIRE_INSTALL_DIR:-$HOME/.local/bin}"

# --- skip when already up to date --------------------------------------------
# Installing the latest is a no-op when the installed vaire is already at or above
# it; a pinned VAIRE_VERSION always installs (that is how an install is repaired).
if [ -z "$pinned" ] && [ -x "$install_dir/$BIN" ]; then
  installed="$("$install_dir/$BIN" --version 2>/dev/null || true)"
  installed="${installed##* }"
  if [ -n "$installed" ] && ver_ge "$installed" "$version"; then
    ok "$BIN $installed is already installed at $install_dir/$BIN (latest release: $version) — nothing to do."
    exit 0
  fi
fi

printf '%s\n' "${bold}Installing ${BIN} ${version}${reset} ${dim}(${target})${reset}"
info "  from $url"
info "  to   $install_dir"

# --- download + extract ------------------------------------------------------
tmp="$(mktemp -d 2>/dev/null || mktemp -d -t vaire)"
trap 'rm -rf "$tmp"' EXIT INT TERM

dl "$url" "$tmp/$asset" || die "download failed: $url"

# --- verify ------------------------------------------------------------------
# The release publishes SHA256SUMS next to the archives. Verify before extracting:
# HTTPS authenticates the transport, not the artifact, so on its own it is no defence
# against a replaced release asset. Set VAIRE_SKIP_CHECKSUM=1 to bypass deliberately.
if [ "${VAIRE_SKIP_CHECKSUM:-0}" = "1" ]; then
  info "  skipping checksum verification (VAIRE_SKIP_CHECKSUM=1)"
else
  sums_url="https://github.com/${REPO}/releases/download/${version}/SHA256SUMS"
  if dl "$sums_url" "$tmp/SHA256SUMS" 2>/dev/null; then
    # Exact filename match on field 2 (stripping sha256sum's binary-mode '*' marker), not a
    # substring search — a sibling asset like "<asset>.sig" would otherwise also match.
    expected="$(awk -v a="$asset" '{ sub(/^\*/, "", $2); if ($2 == a) { print $1; exit } }' "$tmp/SHA256SUMS")"
    [ -n "$expected" ] || die "no checksum for $asset in SHA256SUMS"
    if command -v sha256sum >/dev/null 2>&1; then
      actual="$(sha256sum "$tmp/$asset" | awk '{print $1}')"
    elif command -v shasum >/dev/null 2>&1; then
      actual="$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')"
    else
      die "need sha256sum or shasum to verify the download (or set VAIRE_SKIP_CHECKSUM=1)"
    fi
    [ "$actual" = "$expected" ] || die "checksum mismatch for $asset
  expected $expected
  actual   $actual
Refusing to install. This archive is not the one this release published."
    info "  checksum ok"
  else
    # Releases published before SHA256SUMS existed have nothing to verify against.
    info "  no SHA256SUMS published for $version — skipping verification"
  fi
fi

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
