#!/bin/bash
set -euo pipefail

VERSION="${1:?Usage: ./scripts/release.sh <version> (e.g. 0.3.0)}"

# Strip leading 'v' if provided
VERSION="${VERSION#v}"

TAG="v${VERSION}"

git pull

# Check for clean working tree
if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "Error: Working tree is not clean. Commit or stash changes first."
  exit 1
fi

# Check if tag already exists
if git rev-parse "$TAG" >/dev/null 2>&1; then
  echo "Error: Tag $TAG already exists"
  exit 1
fi

echo "Releasing $VERSION..."

echo "→ Ensuring cargo-edit is installed (force avoids a shadowing cargo-set-version binary)"
cargo install cargo-edit --force --locked

echo "→ Updating Cargo.toml versions to $VERSION"
cargo set-version "$VERSION"

echo "→ Updating Python package version to $VERSION"
PYPROJECT="packages/fff-python/pyproject.toml"
PYINIT="packages/fff-python/src/fff/__init__.py"
UVLOCK="packages/fff-python/uv.lock"

sed_inplace() {
  local file="$1"; shift
  local tmp
  tmp="$(mktemp)"
  sed "$@" "$file" >"$tmp"
  mv "$tmp" "$file"
}

sed_inplace "$PYPROJECT" -e 's/^version = ".*"$/version = "'"$VERSION"'"/'
sed_inplace "$PYINIT" -e 's/^__version__ = ".*"$/__version__ = "'"$VERSION"'"/'

# uv.lock has a `version` line per package; only touch the one directly
# after the `fff-search` package entry. Multi `-e` keeps the `{ }` block
# portable across BSD and GNU sed.
if [ -f "$UVLOCK" ]; then
  sed_inplace "$UVLOCK" \
    -e '/^name = "fff-search"$/{' \
    -e 'n' \
    -e 's/^version = ".*"$/version = "'"$VERSION"'"/' \
    -e '}'
fi

# Fail loudly if any substitution did not land (mirrors the old guard).
grep -q "^version = \"$VERSION\"$" "$PYPROJECT" || { echo "Error: failed to update version in $PYPROJECT"; exit 1; }
grep -q "^__version__ = \"$VERSION\"$" "$PYINIT" || { echo "Error: failed to update version in $PYINIT"; exit 1; }
if [ -f "$UVLOCK" ]; then
  grep -A1 '^name = "fff-search"$' "$UVLOCK" | grep -q "^version = \"$VERSION\"$" \
    || { echo "Error: failed to update fff-search version in $UVLOCK"; exit 1; }
fi

git add -A
git commit -m "chore: release $VERSION"

echo "→ Creating tag $TAG"
git tag -a "$TAG" -m "Release $VERSION"

echo "→ Pushing to origin"
git push origin
git push origin "$TAG"

echo ""
echo "Release $VERSION created and pushed."
echo "CI will build and publish: https://github.com/dmtrKovalenko/fff.nvim/actions"
