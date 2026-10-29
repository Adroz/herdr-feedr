#!/bin/sh
# herdr plugin build step (herdr-plugin.toml's [[build]]): puts the `feedr` binary that the
# [[panes]] entry and the scripts/open-feedr.sh / scripts/toggle-feedr.sh launchers run at
# target/release/feedr. herdr runs this once at `plugin link`/`plugin install` time, and
# again on reinstall-to-update. Modeled on herdr-file-viewer's fetch-or-build.sh.
#
# Fast path: download the prebuilt `herdr-feedr-<triple>` published for the version this
# checkout's Cargo.toml declares (.github/workflows/release.yml), verify it against the
# release's SHA256SUMS, and install it. The match is by version, not commit: a checkout
# ahead of its release still gets that release's binary.
# Fallback: on any miss (unmapped platform, no curl/wget, no release for this version,
# checksum mismatch, no sha-256 tool) say why and `cargo build --release` instead, so
# installing never needs more than it did before prebuilts existed.
#
# FEEDR_FROM_SOURCE=1 skips the fast path (local development: build what's checked out).
# FEEDR_REPO_ROOT / FEEDR_RELEASE_BASE_URL exist for tests/fetch_or_build.rs.
set -u

repo="Adroz/herdr-feedr"

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root="${FEEDR_REPO_ROOT:-$script_dir/..}"
base_url="${FEEDR_RELEASE_BASE_URL:-https://github.com/$repo/releases/download}"
out="$repo_root/target/release/feedr"

have() { command -v "$1" >/dev/null 2>&1; }

download() { # download <url> <dest>
  if have curl; then
    curl -fsSL -o "$2" "$1"
  elif have wget; then
    wget -q -O "$2" "$1"
  else
    return 127
  fi
}

sha256_of() { # prints the hex digest of file $1
  if have sha256sum; then
    sha256sum "$1" | awk '{print $1}'
  elif have shasum; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    return 127
  fi
}

miss() {
  echo "herdr-feedr: $1 — building from source instead." >&2
}

# Install the verified prebuilt at $out, or explain the miss and return 1.
fetch_prebuilt() {
  os=$(uname -s 2>/dev/null || echo unknown)
  arch=$(uname -m 2>/dev/null || echo unknown)
  triple=""
  case "$os/$arch" in
    Darwin/arm64 | Darwin/aarch64) triple="aarch64-apple-darwin" ;;
    Darwin/x86_64 | Darwin/amd64) triple="x86_64-apple-darwin" ;;
    Linux/x86_64 | Linux/amd64) triple="x86_64-unknown-linux-musl" ;;
  esac
  [ -n "$triple" ] || { miss "no prebuilt binary for $os/$arch"; return 1; }

  version=$(grep -E '^version *= *"' "$repo_root/Cargo.toml" 2>/dev/null | head -n 1 |
    sed -E 's/^version *= *"([^"]+)".*/\1/')
  [ -n "$version" ] || { miss "could not read the version from Cargo.toml"; return 1; }

  asset="herdr-feedr-$triple"
  tmpdir=$(mktemp -d 2>/dev/null) || { miss "could not create a temp dir"; return 1; }
  trap 'rm -rf "$tmpdir"' EXIT

  download "$base_url/v$version/$asset" "$tmpdir/$asset" ||
    { miss "no prebuilt $asset for v$version"; return 1; }
  download "$base_url/v$version/SHA256SUMS" "$tmpdir/SHA256SUMS" ||
    { miss "no SHA256SUMS for v$version"; return 1; }

  # coreutils writes `<hex>  <name>` (text mode) or `<hex> *<name>` (binary mode).
  expected=$(grep -E "^[0-9a-f]{64} [ *]$asset\$" "$tmpdir/SHA256SUMS" | awk '{print $1}' | head -n 1)
  [ -n "$expected" ] || { miss "no checksum listed for $asset"; return 1; }
  actual=$(sha256_of "$tmpdir/$asset") || { miss "no sha-256 tool (sha256sum/shasum)"; return 1; }
  [ "$actual" = "$expected" ] ||
    { miss "checksum mismatch for $asset (expected $expected, got $actual)"; return 1; }

  chmod +x "$tmpdir/$asset"
  mkdir -p "$(dirname "$out")"
  mv -f "$tmpdir/$asset" "$out" || { miss "could not install the binary at $out"; return 1; }
  echo "herdr-feedr: installed prebuilt v$version ($triple), SHA-256 verified."
}

build_from_source() {
  # herdr may run this without ~/.cargo/bin on PATH (e.g. launched from a GUI).
  [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
  if ! have cargo; then
    echo "herdr-feedr: cargo not found on PATH." >&2
    echo "herdr-feedr: install Rust (https://rustup.rs) and re-run 'herdr plugin install'/'herdr plugin link'." >&2
    exit 1
  fi
  (cd "$repo_root" && cargo build --release) || exit 1
}

if [ "${FEEDR_FROM_SOURCE:-}" = 1 ]; then
  build_from_source
else
  fetch_prebuilt || build_from_source
fi

# Refresh an already-installed copy of the agent skill so herdr's
# reinstall-to-update flow updates the skill along with the binary (SPEC §5).
# --refresh-only never *creates* a copy: installing a plugin must not write into
# $HOME unasked. First install stays explicit —
#   npx skills add Adroz/herdr-feedr -g   (or)   feedr skill install
"$out" skill install --refresh-only
