#!/bin/sh
# Check scripts/check-release-tag.sh with LF and CRLF manifests: the Windows
# runner checks files out with CRLF line endings.
set -eu

check=$(cd "$(dirname "$0")" && pwd)/check-release-tag.sh
t=$(mktemp -d)
trap 'rm -rf "$t"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }

printf '[package]\nname = "lumen"\nversion = "0.1.0"\n\n[dependencies]\nfoo = { version = "1" }\n' >"$t/lf.toml"
sed 's/$/\r/' "$t/lf.toml" >"$t/crlf.toml"

for manifest in lf crlf; do
    "$check" v0.1.0 "$t/$manifest.toml" >/dev/null || fail "$manifest: v0.1.0 refused"
    for tag in v0.2.0 0.1.0 v0.1.0-rc1 v; do
        if "$check" "$tag" "$t/$manifest.toml" 2>/dev/null; then fail "$manifest: $tag accepted"; fi
    done
done
if "$check" 2>/dev/null; then fail "no tag accepted"; fi

echo "check-release-tag.sh: all checks passed"
