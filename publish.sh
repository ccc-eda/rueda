#!/usr/bin/env bash
# Publish the workspace's crates to crates.io (every crate but rueda-wasm, the web core).
#
#   ./publish.sh --dry-run   package and build every crate as crates.io would, upload nothing
#   ./publish.sh             the checks below, the dry run, then (after a yes) the upload
#
# Before uploading it checks that the tree has no uncommitted changes, that every workspace
# dependency's version equals workspace.package.version, that the tag vMAJOR.MINOR is in HEAD's
# history (commits after it are listed) and that HEAD and the tag are pushed (crates.io keeps a
# version forever: publish what GitHub has).
# A new version: change version in Cargo.toml (all eleven places), write docs/release/vX.Y.md,
# commit, tag vX.Y, push, then ./publish.sh. Needs `cargo login` once.
set -euo pipefail
cd "$(dirname "$0")"

DRY=false
case "${1:-}" in
    --dry-run) DRY=true ;;
    "") ;;
    *) echo "usage: $0 [--dry-run]"; exit 2 ;;
esac

step() { echo; echo "== $*"; }
die() { echo "error: $*" >&2; exit 1; }

VERSION=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)
[ -n "$VERSION" ] || die "no version in Cargo.toml's [workspace.package]"
TAG=v$(echo "$VERSION" | cut -d. -f1,2)
echo "rueda $VERSION (tag $TAG)"

step "versions"
bad=$(grep -E 'path = "[^"]+", version = "' Cargo.toml | grep -v "version = \"$VERSION\"" || true)
[ -z "$bad" ] || die "workspace dependencies not at $VERSION:"$'\n'"$bad"
echo "every workspace dependency is $VERSION"

if ! $DRY; then
    step "git"
    [ -z "$(git status --porcelain --untracked-files=no)" ] || die "uncommitted changes (git status)"
    git rev-parse -q --verify "refs/tags/$TAG" > /dev/null || die "no tag $TAG: git tag -a $TAG -m \"$TAG: ...\""
    git merge-base --is-ancestor "$TAG" HEAD || die "tag $TAG is not in HEAD's history"
    after=$(git log --oneline "$TAG..HEAD")
    [ -z "$after" ] || { echo "commits after $TAG (they are published too, as $VERSION):"; echo "$after" | sed 's/^/  /'; }
    git fetch -q origin || die "git fetch origin failed"
    branch=$(git rev-parse --abbrev-ref HEAD)
    [ "$(git rev-parse HEAD)" = "$(git rev-parse "origin/$branch" 2> /dev/null || true)" ] || die "HEAD is not pushed: git push"
    git ls-remote --exit-code --tags origin "refs/tags/$TAG" > /dev/null || die "tag $TAG is not pushed: git push origin $TAG"
    echo "HEAD and $TAG are committed, tagged and pushed"
fi

step "dry run (package and build every crate)"
cargo publish --workspace --exclude rueda-wasm --dry-run
$DRY && { echo; echo "dry run passed: ./publish.sh uploads rueda $VERSION"; exit 0; }

step "upload"
read -r -p "publish rueda $VERSION to crates.io (it cannot be taken back)? [y/N] " ok
[ "$ok" = y ] || [ "$ok" = Y ] || { echo "not published"; exit 1; }
cargo publish --workspace --exclude rueda-wasm
echo
echo "published rueda $VERSION: cargo install rueda"
