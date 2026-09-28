#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd -P)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
cd "$scratch"
git init -q
git config user.name CI
git config user.email ci@example.test
mkdir -p rustcode/tui
printf 'version = "1.0.0"\n' > Cargo.toml
printf 'version = "1.0.0"\n' > rustcode/tui/Cargo.toml
printf 'version = "1.0.0"\n' > Cargo.lock
printf '# Changelog\n' > CHANGELOG.md
git add .
git commit -qm baseline
base="$(git rev-parse HEAD)"

printf 'version = "1.0.1"\n' > Cargo.toml
printf 'version = "1.0.1"\n' > rustcode/tui/Cargo.toml
printf 'version = "1.0.1"\n' > Cargo.lock
printf '# Changelog\n1.0.1\n' > CHANGELOG.md
git add .
git commit -qm release
bash "$script_dir/ci-version-only.sh" "$base"
echo 'ok - release version edits skip compiled checks'

printf 'dependency = "2"\n' >> Cargo.toml
git add .
git commit -qm dependency
if bash "$script_dir/ci-version-only.sh" "$base"; then exit 1; fi
echo 'ok - dependency edits require compiled checks'

git checkout -q HEAD~1
mkdir -p rustcode/tui/src
printf 'fn main() {}\n' > rustcode/tui/src/main.rs
git add .
git commit -qm source
if bash "$script_dir/ci-version-only.sh" "$base"; then exit 1; fi
echo 'ok - source edits require compiled checks'
