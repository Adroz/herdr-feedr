#!/bin/sh
# Release gate (.github/workflows/release.yml): fail unless the pushed tag names the version
# both Cargo.toml and herdr-plugin.toml declare, so a release's binaries always match the
# source `herdr plugin install` clones — fetch-or-build.sh picks its download by that version.
#
# Usage: scripts/check-release-version.sh <tag>   (e.g. v0.3.0)
# FEEDR_REPO_ROOT exists for tests/fetch_or_build.rs.
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root="${FEEDR_REPO_ROOT:-$script_dir/..}"

tag="${1:?usage: scripts/check-release-version.sh <tag>}"
want="${tag#v}"

# The first top-level `version = "..."` line: the package's in Cargo.toml, the plugin's in
# herdr-plugin.toml (dependency versions are inline tables, never at line start).
version_of() {
  grep -E '^version *= *"' "$repo_root/$1" | head -n 1 | sed -E 's/^version *= *"([^"]+)".*/\1/'
}

status=0
for manifest in Cargo.toml herdr-plugin.toml; do
  declared=$(version_of "$manifest")
  if [ "$declared" != "$want" ]; then
    echo "tag $tag ($want) != $manifest version ($declared)" >&2
    status=1
  fi
done
exit "$status"
