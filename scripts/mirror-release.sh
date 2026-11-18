#!/bin/sh
set -eu
tag="${1:?usage: scripts/mirror-release.sh v<version>}"
bucket="lost-scores-releases"
mirror="https://lost.lemon4ik.kz/dl"
key=$(sed -n 's/^pub const PUBLIC_KEY: &str = "\(.*\)";$/\1/p' crates/lost-scores-app/src/updates.rs)
[ -n "$key" ] || { echo "cannot read PUBLIC_KEY from the code" >&2; exit 1; }
[ "$(gh release view "$tag" --json isDraft --jq .isDraft)" = "false" ] || { echo "publish $tag before mirroring it" >&2; exit 1; }
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/files"
gh release download "$tag" --dir "$work/files"
gh release view "$tag" --json url,publishedAt,assets > "$work/release.json"
cd "$work/files"
[ -f SHA256SUMS.minisig ] || { echo "$tag is not signed yet" >&2; exit 1; }
minisign -Vm SHA256SUMS -P "$key" > /dev/null
sha256sum --check --strict --quiet SHA256SUMS
for file in *; do
  npx --yes wrangler@4 r2 object put "$bucket/$tag/$file" --file "$file" --content-type application/octet-stream --remote
done
python3 -I -c '
import json, sys
tag, mirror = sys.argv[1], sys.argv[2]
release = json.load(sys.stdin)
assets = [{"name": a["name"], "size": a["size"], "browser_download_url": mirror + "/" + tag + "/" + a["name"]} for a in release["assets"]]
print(json.dumps({"tag_name": tag, "html_url": release["url"], "published_at": release["publishedAt"], "assets": assets}))
' "$tag" "$mirror" < "$work/release.json" > "$work/latest.json"
npx --yes wrangler@4 r2 object put "$bucket/latest.json" --file "$work/latest.json" --content-type application/json --remote
echo "mirrored $tag to $mirror"
