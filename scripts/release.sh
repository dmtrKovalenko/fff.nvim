#!/bin/bash
set -euo pipefail

VERSION="${1:?Usage: ./scripts/release.sh <version> (e.g. 0.3.0)}"

# Strip leading 'v' if provided
VERSION="${VERSION#v}"

TAG="v${VERSION}"

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

echo "→ Updating Cargo.toml versions to $VERSION"
cargo set-version "$VERSION" 2>/dev/null || {
  echo "cargo-edit not found, installing..."
  cargo install cargo-edit
  cargo set-version "$VERSION"
k

# Commit version bump
git add -A
git commit -m "chore: release $VERSION"

# Create annotated tag
echo "→ Creating tag $TAG"
git tag -a "$TAG" -m "Release $VERSION"

# Push commit and tag
echo "→ Pushing to origin"
git push origin "$BRANCH"
git push origin "$TAG"

echo ""
echo "Release $VERSION created and pushed."
echo "CI will build and publish: https://github.com/dmtrKovalenko/fff.nvim/actions"
