#!/bin/sh
# 別プロセス・別ビルドで生成した値と診断を、バイト単位で比較する。
set -eu
cd "$(dirname "$0")/.."
snapshot_dir=$(mktemp -d)
trap 'rm -rf "$snapshot_dir"' EXIT HUP INT TERM
RIN_DETERMINISM_OUTPUT="$snapshot_dir/debug.txt" \
    cargo test --locked -p rin --test spec_cases determinism::write_snapshot -- --exact --ignored
RIN_DETERMINISM_OUTPUT="$snapshot_dir/release.txt" \
    cargo test --locked --release -p rin --test spec_cases determinism::write_snapshot -- --exact --ignored
cmp "$snapshot_dir/debug.txt" "$snapshot_dir/release.txt"
printf 'Determinism: Debug/Release snapshots match (%s lines).\n' "$(wc -l < "$snapshot_dir/debug.txt" | tr -d ' ')"
# CIではプラットフォーム間でも同じスナップショットか照合する。
if [ -n "${GITHUB_OUTPUT:-}" ]; then
    shasum -a 256 "$snapshot_dir/debug.txt" | awk '{print "digest=" $1}' >> "$GITHUB_OUTPUT"
fi
