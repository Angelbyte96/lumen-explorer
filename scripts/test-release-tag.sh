#!/bin/sh
# Check scripts/check-release-tag.sh with LF and CRLF manifests (the Windows
# runner checks files out with CRLF line endings) and
# scripts/check-annotated-tag.sh with light and annotated tags.
set -eu

dir=$(cd "$(dirname "$0")" && pwd)
check=$dir/check-release-tag.sh
annotated=$dir/check-annotated-tag.sh
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

# A throwaway repository with one light and one annotated tag.
git init -q "$t/repo"
(
    cd "$t/repo"
    git -c user.name=t -c user.email=t@t commit -q --allow-empty -m init
    git tag v0.1.0
    git -c user.name=t -c user.email=t@t tag -a v0.2.0 -m "v0.2.0"
) || fail "could not build the test repository"

(cd "$t/repo" && "$annotated" v0.2.0 >/dev/null) || fail "annotated v0.2.0 refused"
if out=$(cd "$t/repo" && "$annotated" v0.1.0 2>&1); then fail "light v0.1.0 accepted"; fi
case $out in *"::error::"*"v0.1.0"*"annotated"*) ;; *) fail "light tag message: $out" ;; esac
# Exit codes: 1 for a tag that is missing or light, 2 for a wrong call.
code() { ("$@" >/dev/null 2>&1) && echo 0 || echo $?; }
[ "$(cd "$t/repo" && code "$annotated" v0.1.0)" = 1 ] || fail "light tag: exit code is not 1"
[ "$(cd "$t/repo" && code "$annotated" v9.9.9)" = 1 ] || fail "missing tag: exit code is not 1"
[ "$(cd "$t/repo" && code "$annotated")" = 2 ] || fail "no tag: exit code is not 2"
[ "$(cd "$t/repo" && code "$annotated" v0.2.0 extra)" = 2 ] || fail "extra argument: exit code is not 2"
[ "$(code "$check")" = 2 ] || fail "check-release-tag.sh without a tag: exit code is not 2"
out=$(cd "$t/repo" && "$annotated" v9.9.9 2>&1) || true
case $out in *"::error::"*"v9.9.9"*"not found"*) ;; *) fail "missing tag message: $out" ;; esac
out=$(cd "$t/repo" && "$annotated" v0.1.0 2>&1) || true
case $out in *"git tag -a"*) ;; *) fail "light tag hint missing: $out" ;; esac

# actions/checkout fetches the commit into refs/tags/X, which makes an annotated
# tag light: the workflow fetches the tag again before checking it.
sha=$(git -C "$t/repo" rev-parse 'v0.2.0^{commit}')
git init -q "$t/ci"
(
    cd "$t/ci"
    git remote add origin "file://$t/repo"
    git fetch -q --no-tags origin "+$sha:refs/tags/v0.2.0"
    if "$annotated" v0.2.0 2>/dev/null; then exit 1; fi
    git fetch -q --no-tags --force origin "refs/tags/v0.2.0:refs/tags/v0.2.0"
    "$annotated" v0.2.0 >/dev/null
) || fail "annotated tag not recovered after the checkout-style fetch"

echo "check-release-tag.sh and check-annotated-tag.sh: all checks passed"
