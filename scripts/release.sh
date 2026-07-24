#!/bin/bash

# Usage: ./scripts/release.sh <new_version>
NEW_VERSION=$1
PREV_TAG=$(git describe --tags --abbrev=0)

# 1. Update Cargo.toml
sed -i "s/version = \"[^\"]*\"/version = \"${NEW_VERSION#v}\"/" Cargo.toml

# 2. Generate Changelog entries
COMMITS=$(git log ${PREV_TAG}..HEAD --oneline --no-merges)
{
  echo "## [${NEW_VERSION}] - $(date +%Y-%m-%d)"
  echo "$COMMITS"
  echo ""
  cat CHANGELOG.md
} > CHANGELOG.md.tmp && mv CHANGELOG.md.tmp CHANGELOG.md

# 3. Commit, Tag, Push
git add Cargo.toml CHANGELOG.md
git commit -m "chore: release ${NEW_VERSION}"
git tag ${NEW_VERSION}
git push origin main --tags
