---
name: release-automation
description: Automates version bumping, changelog generation, and pushing tags/releases.
---
Use this skill to perform project releases. It executes `scripts/release.sh <version>` to:
1. Update Cargo.toml version.
2. Generate changelog entries from git logs.
3. Commit, tag, and push changes.

Example: `run_command scripts/release.sh v0.1.18`