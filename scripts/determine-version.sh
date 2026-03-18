#!/bin/bash
# Determines the release version based on git context.
#
# Tagged release (refs/tags/v*) → uses the tag version (e.g. v0.2.4 → 0.2.4)
# Push to main                  → nightly prerelease   (e.g. 0.2.5-nightly.abc1234)
# Other (PR / dev branch)       → dev prerelease       (e.g. 0.2.5-dev.abc1234)
#
# For prerelease builds the patch version is bumped so that the resulting
# semver is strictly greater than the current Cargo.toml version.
# (In semver 0.2.4-nightly.x < 0.2.4, so we need 0.2.5-nightly.x > 0.2.4.)
#
# Outputs (appended to $GITHUB_OUTPUT when running in CI):
#   version     – semver string
#   npm_tag     – npm dist-tag  (latest | nightly | dev)
#   is_release  – "true" for tagged releases, "false" otherwise
#
# Can also be run locally for debugging:
#   GITHUB_REF=refs/tags/v0.3.0 ./scripts/determine-version.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Single source of truth: base version lives in fff-core
base_version=$(grep '^version' "$REPO_ROOT/crates/fff-core/Cargo.toml" \
  | head -1 \
  | sed 's/version = "\(.*\)"/\1/')

short_sha=$(git -C "$REPO_ROOT" rev-parse --short HEAD)

# Bump the patch component: 0.2.4 → 0.2.5
IFS='.' read -r major minor patch <<< "$base_version"
next_patch_version="${major}.${minor}.$((patch + 1))"

ref="${GITHUB_REF:-}"

if [[ "$ref" == refs/tags/v* ]]; then
  # Tagged release – strip the leading "v"
  version="${ref#refs/tags/v}"
  npm_tag="latest"
  is_release="true"
elif [[ "$ref" == "refs/heads/main" || "$ref" == "refs/heads/node" ]]; then
  version="${next_patch_version}-nightly.${short_sha}"
  npm_tag="nightly"
  is_release="false"
else
  version="${next_patch_version}-dev.${short_sha}"
  npm_tag="dev"
  is_release="false"
fi

echo "version=${version}"
echo "npm_tag=${npm_tag}"
echo "is_release=${is_release}"

# Write to GITHUB_OUTPUT when running in CI
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  echo "version=${version}"   >> "$GITHUB_OUTPUT"
  echo "npm_tag=${npm_tag}"   >> "$GITHUB_OUTPUT"
  echo "is_release=${is_release}" >> "$GITHUB_OUTPUT"
fi
