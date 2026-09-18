#!/bin/sh
# herdr plugin build step (herdr-plugin.toml's [[build]]): compiles the `feedr` binary
# that the [[panes]] entry and the scripts/open-feedr.sh / scripts/toggle-feedr.sh
# launchers run. herdr runs this once at `plugin link`/`plugin install` time, and again
# on reinstall-to-update.
#
# v1 always builds from source — there is no prebuilt-binary fetch path yet. TODO: once
# herdr-feedr ships tagged GitHub releases, add a fetch-or-build.sh modeled on
# herdr-file-viewer's (~/.config/herdr/plugins/github/herdr-file-viewer-*/scripts/
# fetch-or-build.sh): try downloading a prebuilt binary matching this manifest's version
# + platform, verify its SHA-256, and fall back to `cargo build --release` only on a miss
# (no matching release, download/checksum failure, unsupported platform). Until then,
# installing this plugin requires a Rust toolchain.
set -eu

if ! command -v cargo >/dev/null 2>&1; then
  echo "herdr-feedr: cargo not found on PATH." >&2
  echo "herdr-feedr: install Rust (https://rustup.rs) and re-run 'herdr plugin install'/'herdr plugin link'." >&2
  exit 1
fi

script_dir="$(cd "$(dirname "$0")" && pwd)"
cd "$script_dir/.."

cargo build --release

# Refresh an already-installed copy of the agent skill so herdr's
# reinstall-to-update flow updates the skill along with the binary (SPEC §5).
# --refresh-only never *creates* a copy: installing a plugin must not write into
# $HOME unasked. First install stays explicit —
#   npx skills add Adroz/herdr-feedr -g   (or)   feedr skill install
./target/release/feedr skill install --refresh-only
