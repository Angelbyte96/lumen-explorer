#!/bin/sh
# Fail unless TAG is an annotated tag (git tag -a) in the current repository.
# Usage: scripts/check-annotated-tag.sh TAG
# Used by .github/workflows/release.yml; checked by scripts/test-release-tag.sh.
set -eu

[ $# -eq 1 ] || { echo "Usage: $0 TAG" >&2; exit 2; }
tag=$1

if ! kind=$(git cat-file -t "refs/tags/$tag" 2>/dev/null); then
    echo "::error::Tag $tag not found in this repository" >&2
    exit 1
fi
if [ "$kind" != tag ]; then
    echo "::error::Tag $tag is not annotated; create it with: git tag -a $tag -m \"$tag\"" >&2
    exit 1
fi
echo "Tag $tag is annotated"
