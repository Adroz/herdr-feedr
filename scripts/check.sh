#!/bin/sh
# The checks CI runs (.github/workflows/ci.yml), in the same order. Run before
# committing; the pre-commit hook (.githooks/pre-commit) runs `--fast`.
#
#   scripts/check.sh          fmt, clippy, tests
#   scripts/check.sh --fast   fmt and clippy only
#
# A failing insta snapshot leaves a .snap.new beside it: review with
# `cargo insta review` (cargo install cargo-insta), or set INSTA_UPDATE=new and
# inspect the .snap.new files by hand.
set -eu
cd "$(dirname "$0")/.."

cargo fmt --check
cargo clippy --all-targets --quiet -- -D warnings
[ "${1:-}" = "--fast" ] || cargo test --quiet
