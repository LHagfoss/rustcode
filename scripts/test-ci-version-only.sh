#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd -P)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
cd "$scratch"
git init -q
git config user.name CI
git config user.email ci@example.test
mkdir -p rustcode/cli harness apps/desktop/src-tauri
printf 'version = "1.0.0"\n' > Cargo.toml
printf 'version = "1.0.0"\n' > rustcode/cli/Cargo.toml
printf 'version = "1.0.0"\n' > harness/Cargo.toml
printf 'version = "1.0.0"\n' > Cargo.lock
printf 'version = "1.0.0"\n' > apps/desktop/src-tauri/Cargo.lock
printf '# Changelog\n' > CHANGELOG.md
git add .
git commit -qm baseline
base="$(git rev-parse HEAD)"

printf 'version = "1.0.1"\n' > Cargo.toml
printf 'version = "1.0.1"\n' > rustcode/cli/Cargo.toml
printf 'version = "1.0.1"\n' > harness/Cargo.toml
printf 'version = "1.0.1"\n' > Cargo.lock
printf 'version = "1.0.1"\n' > apps/desktop/src-tauri/Cargo.lock
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
printf 'version = "1.0.1"\n[[package]]\nname = "tauri"\nversion = "2.0.0"\n' > apps/desktop/src-tauri/Cargo.lock
git add .
git commit -qm desktop-dependency
if bash "$script_dir/ci-version-only.sh" "$base"; then exit 1; fi
echo 'ok - desktop lockfile dependency edits require compiled checks'

git checkout -q HEAD~1
mkdir -p harness/src
printf 'fn main() {}\n' > harness/src/main.rs
git add .
git commit -qm source
if bash "$script_dir/ci-version-only.sh" "$base"; then exit 1; fi
echo 'ok - source edits require compiled checks'

# The path-only mode the CI filter job uses before it has a diff.
prefilter() { bash "$script_dir/ci-version-only.sh" --prefilter; }
printf 'CHANGELOG.md\nCargo.toml\nCargo.lock\nrustcode/cli/Cargo.toml\nharness/Cargo.toml\napps/desktop/src-tauri/Cargo.lock\n' | prefilter
echo 'ok - release metadata paths pass the prefilter'

if printf 'CHANGELOG.md\nharness/src/main.rs\n' | prefilter; then exit 1; fi
echo 'ok - a source path fails the prefilter'

if printf 'docs/readme.md\n' | prefilter; then exit 1; fi
echo 'ok - an unrelated path fails the prefilter'

if prefilter </dev/null; then exit 1; fi
echo 'ok - an empty path list fails the prefilter'

# The version-consistency check a release PR runs instead of the test suite.
consistency="$(mktemp -d)"
trap 'rm -rf "$scratch" "$consistency"' EXIT
mkdir -p "$consistency/harness" "$consistency/rustcode/cli" "$consistency/apps/desktop/src-tauri"
printf '[package]\nname = "rustcode"\nversion = "2.0.0"\n' > "$consistency/Cargo.toml"
printf '[package]\nname = "rustcode-harness"\nversion = "2.0.0"\n' > "$consistency/harness/Cargo.toml"
printf '[package]\nname = "rustcode-cli"\nversion = "2.0.0"\n' > "$consistency/rustcode/cli/Cargo.toml"
cat > "$consistency/Cargo.lock" <<'LOCK'
[[package]]
name = "rustcode"
version = "2.0.0"

[[package]]
name = "rustcode-cli"
version = "2.0.0"

[[package]]
name = "rustcode-cli"
version = "0.1.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "rustcode-harness"
version = "2.0.0"
LOCK
printf '[[package]]\nname = "rustcode-client"\nversion = "2.0.0"\n' > "$consistency/apps/desktop/src-tauri/Cargo.lock"
bash "$script_dir/ci-version-consistency.sh" "$consistency" >/dev/null
echo 'ok - matching versions pass the consistency check'

printf '[[package]]\nname = "rustcode-client"\nversion = "1.9.0"\n' > "$consistency/apps/desktop/src-tauri/Cargo.lock"
if bash "$script_dir/ci-version-consistency.sh" "$consistency" >/dev/null; then exit 1; fi
echo 'ok - a stale desktop lockfile fails the consistency check'

printf '[[package]]\nname = "rustcode-client"\nversion = "2.0.0"\n' > "$consistency/apps/desktop/src-tauri/Cargo.lock"
printf '[package]\nname = "rustcode-cli"\nversion = "1.9.0"\n' > "$consistency/rustcode/cli/Cargo.toml"
if bash "$script_dir/ci-version-consistency.sh" "$consistency" >/dev/null; then exit 1; fi
echo 'ok - a crate left behind fails the consistency check'
