#!/bin/sh
set -eu
tag="${1:?usage: scripts/sign-release.sh v<version>}"
version="${tag#v}"
key=$(sed -n 's/^pub const PUBLIC_KEY: &str = "\(.*\)";$/\1/p' crates/lost-scores-app/src/updates.rs)
[ -n "$key" ] || { echo "cannot read PUBLIC_KEY from the code" >&2; exit 1; }
[ "$(gh release view "$tag" --json isDraft --jq .isDraft)" = "true" ] || { echo "$tag is not a draft release" >&2; exit 1; }
mine=$(git rev-parse "$tag^{commit}")
theirs=$(gh api "repos/{owner}/{repo}/commits/$tag" --jq .sha)
[ "$mine" = "$theirs" ] || { echo "$tag on GitHub is $theirs, your local $tag is $mine" >&2; exit 1; }
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
gh release download "$tag" --dir "$work"
cd "$work"
rm -f SHA256SUMS.minisig
for file in *; do
  [ "$file" = SHA256SUMS ] && continue
  case "$file" in
    *"-$version-"*) ;;
    *) echo "$file is not a file of $tag" >&2; exit 1 ;;
  esac
  grep -q "  $file\$" SHA256SUMS || { echo "$file is not listed in SHA256SUMS" >&2; exit 1; }
done
[ "$(wc -l < SHA256SUMS)" -eq "$(ls | grep -cv '^SHA256SUMS$')" ] || { echo "SHA256SUMS lists files the release does not have" >&2; exit 1; }
sha256sum --check --strict --quiet SHA256SUMS
minisign -Sm SHA256SUMS -t "lost-scores $tag"
minisign -Vm SHA256SUMS -P "$key" > /dev/null
gh release upload "$tag" SHA256SUMS.minisig --clobber
echo "signed $tag"
