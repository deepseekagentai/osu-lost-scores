#!/bin/sh
set -eu
tag="${1:?usage: scripts/sign-release.sh v<version>}"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
gh release download "$tag" --pattern SHA256SUMS --dir "$work"
minisign -Sm "$work/SHA256SUMS" -t "lost-scores $tag"
minisign -Vm "$work/SHA256SUMS" -P "$(sed -n 's/^pub const PUBLIC_KEY: &str = "\(.*\)";$/\1/p' crates/lost-scores-app/src/updates.rs)"
gh release upload "$tag" "$work/SHA256SUMS.minisig" --clobber
echo "signed $tag"
