#!/bin/sh
# Fail unless TAG is "v" + the package version in Cargo.toml.
# Usage: scripts/check-release-tag.sh TAG [path/to/Cargo.toml]
# Used by .github/workflows/release.yml; checked by scripts/test-release-tag.sh.
set -eu

[ $# -ge 1 ] && [ $# -le 2 ] || { echo "Usage: $0 TAG [Cargo.toml]" >&2; exit 2; }
tag=$1
manifest=${2:-$(dirname "$0")/../Cargo.toml}

# Anything after the closing quote is ignored: a CRLF checkout leaves a \r there.
version=$(sed -n 's/^version = "\([^"]*\)".*/\1/p' "$manifest" | head -n 1)
if [ "$tag" != "v$version" ]; then
    echo "::error::Tag $tag does not match Cargo.toml version $version (expected v$version)" >&2
    exit 1
fi
echo "Tag $tag matches Cargo.toml"
