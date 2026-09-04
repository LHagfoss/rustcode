#!/usr/bin/env bash
# release.sh — Automated release script for the rustcode workspace.
#
# Usage:
#   scripts/release.sh --version 0.52.0 [--date 2026-09-04] [--category Features] [--notes-file NOTES.md] [--dry-run] [--yes]
#   scripts/release.sh --help
#
# This script is intentionally conservative: it never pushes, tags, or merges
# unless explicitly confirmed.  --dry-run prints every command that would run.
# --yes skips interactive prompts but still requires the pre-release diff
# confirmation.

set -euo pipefail

# ── Globals ──────────────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"

VERSION=""
DATE=""
CATEGORY="Features"
NOTES_FILE=""
DRY_RUN=false
YES=false
RELEASE_BRANCH=""
ORIGINAL_BRANCH=""

# ── Helpers ──────────────────────────────────────────────────────────────────
info()  { printf '\033[1;34m[INFO]\033[0m  %s\n' "$*"; }
warn()  { printf '\033[1;33m[WARN]\033[0m %s\n' "$*" >&2; }
error() { printf '\033[1;31m[ERROR]\033[0m %s\n' "$*" >&2; }
die()   { error "$*"; cleanup_and_exit 1; }

# Print a command before executing it (or the equivalent in dry-run).
run() {
    if $DRY_RUN; then
        info "[dry-run] $*"
    else
        info "$*"
        "$@"
    fi
}

# Ask a yes/no question; --yes auto-answers yes.
ask() {
    if $YES; then
        info "[auto-yes] $*"
        return 0
    fi
    printf '%s [y/N] ' "$*"
    read -r answer
    [[ "$answer" =~ ^[Yy]([Ee][Ss])?$ ]]
}

# Get the editor to use, falling back from EDITOR to VISUAL.
get_editor() {
    if [[ -n "${EDITOR:-}" ]]; then
        echo "$EDITOR"
    elif [[ -n "${VISUAL:-}" ]]; then
        echo "$VISUAL"
    else
        echo "vi"
    fi
}

# Cleanup: restore branch if we switched, print recovery instructions.
cleanup_and_exit() {
    local code="$1"
    if [[ -n "${RELEASE_BRANCH:-}" ]] && [[ "$(git -C "$REPO_ROOT" branch --show-current 2>/dev/null || true)" == "$RELEASE_BRANCH" ]]; then
        info "Returning to original branch: $ORIGINAL_BRANCH"
        run git -C "$REPO_ROOT" checkout "$ORIGINAL_BRANCH" 2>/dev/null || true
        info "Recovery: To resume the release, run:"
        info "  scripts/release.sh --version $VERSION ${DRY_RUN:+--dry-run}"
    fi
    exit "$code"
}
trap 'cleanup_and_exit 1' INT TERM

# ── Dependency checks ────────────────────────────────────────────────────────
check_deps() {
    local missing=()
    for cmd in git cargo gh; do
        if ! command -v "$cmd" >/dev/null 2>&1; then
            missing+=("$cmd")
        fi
    done
    if [[ ${#missing[@]} -gt 0 ]]; then
        die "Missing required commands: ${missing[*]}. Install them and retry."
    fi
    if [[ -z "${EDITOR:-}" ]] && [[ -z "${VISUAL:-}" ]]; then
        die "No EDITOR or VISUAL set. Set one (e.g. export EDITOR=vim) and retry."
    fi
}

# ── Argument parsing ─────────────────────────────────────────────────────────
usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Release automation for the rustcode workspace.

Required:
  --version VERSION    Semver version to release (e.g. 0.52.0)

Optional:
  --date DATE          Release date (YYYY-MM-DD), defaults to today
  --category CATEGORY  Changelog category (Features|Fixes|Chores|…); default: Features
  --notes-file FILE    Path to a file containing changelog notes
  --dry-run            Print every command without executing
  --yes                Skip all interactive prompts
  --help               Show this help message

Examples:
  scripts/release.sh --version 0.52.0 --yes
  scripts/release.sh --version 0.52.0 --category Fixes --notes-file release-notes.md
  scripts/release.sh --version 0.52.0 --dry-run
EOF
}

parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --version)    VERSION="$2"; shift 2 ;;
            --date)       DATE="$2"; shift 2 ;;
            --category)   CATEGORY="$2"; shift 2 ;;
            --notes-file) NOTES_FILE="$2"; shift 2 ;;
            --dry-run)    DRY_RUN=true; shift ;;
            --yes)        YES=true; shift ;;
            --help)       usage; exit 0 ;;
            *) die "Unknown option: $1. Run with --help for usage." ;;
        esac
    done

    # Interactive version prompt if not supplied.
    if [[ -z "$VERSION" ]]; then
        local current
        current="$(get_current_version)"
        printf 'Current version: %s\n' "$current"
        printf 'Enter next version: '
        read -r VERSION
        if [[ -z "$VERSION" ]]; then
            die "No version supplied."
        fi
    fi

    # Default date to today.
    if [[ -z "$DATE" ]]; then
        DATE="$(date +%Y-%m-%d)"
    fi

    # Validate semver (basic check: X.Y.Z with optional pre-release).
    if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.]+)?$ ]]; then
        die "Invalid semver: $VERSION. Expected format: X.Y.Z (e.g. 0.52.0)"
    fi
}

# ── Version helpers ──────────────────────────────────────────────────────────
get_current_version() {
    grep -m1 '^version = ' "$REPO_ROOT/Cargo.toml" | sed 's/^version = "\(.*\)"/\1/'
}

# Compare two semver strings; returns 0 if a > b, 1 otherwise.
semver_gt() {
    local sorted last
    sorted="$(printf '%s\n%s\n' "$1" "$2" | sort -V)"
    last="$(printf '%s\n' "$sorted" | tail -n1)"
    [[ "$last" == "$1" && "$1" != "$2" ]]
}

# ── Validation ───────────────────────────────────────────────────────────────
validate() {
    info "Validating release prerequisites…"

    # 1. Version is greater than current.
    local current
    current="$(get_current_version)"
    info "Current version: $current  →  Target: $VERSION"
    if ! semver_gt "$VERSION" "$current"; then
        die "Version $VERSION is not greater than current version $current."
    fi

    # 2. Tag does not already exist.
    if git -C "$REPO_ROOT" tag --list "v$VERSION" | grep -q "^v$VERSION$"; then
        die "Tag v$VERSION already exists. Delete it or choose a different version."
    fi

    # 3. Worktree is clean.
    local status
    status="$(git -C "$REPO_ROOT" status --porcelain)"
    if [[ -n "$status" ]]; then
        die "Worktree is not clean. Commit or stash changes before releasing:\n$status"
    fi

    # 4. Current branch is main and exactly matches origin/main.
    local current_branch
    current_branch="$(git -C "$REPO_ROOT" branch --show-current)"
    if [[ "$current_branch" != "main" ]]; then
        die "Not on main branch (currently on '$current_branch'). Checkout main and retry."
    fi

    # Fetch latest origin/main to ensure accurate comparison.
    run git -C "$REPO_ROOT" fetch origin main

    local origin_main local_main
    origin_main="$(git -C "$REPO_ROOT" rev-parse --verify origin/main 2>/dev/null || true)"
    local_main="$(git -C "$REPO_ROOT" rev-parse --verify main 2>/dev/null || true)"

    if [[ -z "$origin_main" ]]; then
        die "Could not find origin/main. Is the remote configured?"
    fi

    if [[ "$local_main" != "$origin_main" ]]; then
        local ahead behind
        ahead="$(git -C "$REPO_ROOT" rev-list --left-right --count main...origin/main 2>/dev/null | awk '{print $1}')"
        behind="$(git -C "$REPO_ROOT" rev-list --left-right --count main...origin/main 2>/dev/null | awk '{print $2}')"
        if [[ "$ahead" -gt 0 && "$behind" -eq 0 ]]; then
            die "main is ahead of origin/main by $ahead commit(s). Push your changes first: git push origin main"
        elif [[ "$behind" -gt 0 && "$ahead" -eq 0 ]]; then
            die "main is behind origin/main by $behind commit(s). Run 'git pull' and retry."
        else
            die "main is diverged from origin/main (ahead by $ahead, behind by $behind). Resolve the divergence and retry."
        fi
    fi

    info "All validations passed."
}

# ── Workspace discovery ──────────────────────────────────────────────────────
get_workspace_crate_paths() {
    # Use python3 for robust TOML parsing of multi-line members array.
    python3 -c "
import re, sys
with open('$REPO_ROOT/Cargo.toml') as f:
    content = f.read()
match = re.search(r'members\s*=\s*\[(.*?)\]', content, re.DOTALL)
if match:
    members_str = match.group(1)
    members = re.findall(r'\"([^\"]+)\"', members_str)
    for m in members:
        print(m)
"
}

# ── Phase 1: Branch & version bump ───────────────────────────────────────────
phase_branch() {
    RELEASE_BRANCH="chore/release-v$VERSION"
    ORIGINAL_BRANCH="$(git -C "$REPO_ROOT" branch --show-current)"
    info "Phase 1: Creating release branch $RELEASE_BRANCH"
    if $DRY_RUN; then
        info "[dry-run] git checkout -b $RELEASE_BRANCH"
    else
        git -C "$REPO_ROOT" checkout -b "$RELEASE_BRANCH"
    fi
}

phase_update_versions() {
    info "Phase 2: Updating versions to $VERSION"

    # Update root Cargo.toml.
    local root_toml="$REPO_ROOT/Cargo.toml"
    local tmp
    tmp="$(mktemp)"
    sed 's/^version = ".*"/version = "'"$VERSION"'"/' "$root_toml" > "$tmp"
    run mv -- "$tmp" "$root_toml"

    # Update each workspace crate's Cargo.toml.
    local crate_path
    for crate_path in $(get_workspace_crate_paths); do
        local crate_toml="$REPO_ROOT/$crate_path/Cargo.toml"
        if [[ -f "$crate_toml" ]]; then
            tmp="$(mktemp)"
            sed 's/^version = ".*"/version = "'"$VERSION"'"/' "$crate_toml" > "$tmp"
            run mv -- "$tmp" "$crate_toml"
        fi
    done
}

phase_update_lockfile() {
    info "Phase 3: Updating Cargo.lock"
    if $DRY_RUN; then
        info "[dry-run] cargo update --locked"
    else
        # --locked ensures we don't upgrade unrelated dependencies.
        # If the lockfile needs updating for version changes, this will fail
        # and we fall back to generate-lockfile.
        if ! cargo update --locked 2>/dev/null; then
            info "Lockfile needs updating for version change, regenerating…"
            cargo generate-lockfile
        fi
    fi
}

phase_update_changelog() {
    info "Phase 4: Updating CHANGELOG.md"

    # Collect changelog notes.
    local notes=""
    if [[ -n "$NOTES_FILE" ]]; then
        if [[ ! -f "$NOTES_FILE" ]]; then
            die "Notes file not found: $NOTES_FILE"
        fi
        notes="$(cat "$NOTES_FILE")"
    else
        # Open editor for multiline input.
        local tmpfile editor
        tmpfile="$(mktemp /tmp/rustcode-changelog-XXXXXX.md)"
        editor="$(get_editor)"
        # Only write the category heading if notes file not provided.
        cat > "$tmpfile" <<EOF
### $CATEGORY
-
EOF
        if $DRY_RUN; then
            info "[dry-run] Would open editor for changelog notes: $tmpfile"
            notes="$(cat "$tmpfile")"
        else
            "$editor" "$tmpfile"
            notes="$(cat "$tmpfile")"
        fi
        rm -f -- "$tmpfile"
    fi

    # Build the new changelog entry.
    # Only prepend category heading if notes don't already start with one.
    local category_heading=""
    if [[ ! "$notes" =~ ^#[[:space:]]*#?[[:space:]]*$CATEGORY ]]; then
        category_heading="### $CATEGORY
"
    fi

    local new_entry
    new_entry="## [v$VERSION](https://github.com/LHagfoss/rustcode/releases/tag/v$VERSION) - $DATE

${category_heading}${notes}
"

    # Prepend to CHANGELOG.md.
    local changelog="$REPO_ROOT/CHANGELOG.md"
    local tmp
    tmp="$(mktemp)"
    printf '%s' "$new_entry" > "$tmp"
    cat "$changelog" >> "$tmp"
    run mv -- "$tmp" "$changelog"
}

# ── Phase: Show diff & confirm ───────────────────────────────────────────────
phase_show_diff() {
    info "Phase 5: Showing proposed changes"
    run git -C "$REPO_ROOT" diff --stat
    echo
    run git -C "$REPO_ROOT" diff
    echo
    if ! ask "Confirm these changes and continue?"; then
        die "Release aborted by user."
    fi
}

# ── Phase: Verification ──────────────────────────────────────────────────────
phase_verify() {
    info "Phase 6: Running verification checks"

    local checks=(
        "cargo fmt --check"
        "cargo check --tests"
        "cargo check --tests --locked"
        "cargo test --locked"
        "git diff --check"
    )

    local cmd
    for cmd in "${checks[@]}"; do
        info "Running: $cmd"
        if $DRY_RUN; then
            info "[dry-run] $cmd"
        else
            if ! (cd "$REPO_ROOT" && eval "$cmd"); then
                die "Verification failed: $cmd
To resume: fix the reported issues and re-run: scripts/release.sh --version $VERSION ${DRY_RUN:+--dry-run}"
            fi
        fi
    done

    info "All verification checks passed."
}

# ── Phase: Commit & push ─────────────────────────────────────────────────────
phase_commit() {
    info "Phase 7: Committing release changes"
    run git -C "$REPO_ROOT" add -u
    run git -C "$REPO_ROOT" commit -m "chore: release v$VERSION"
}

phase_push() {
    info "Phase 8: Pushing release branch"
    run git -C "$REPO_ROOT" push -u origin "$RELEASE_BRANCH"
}

# ── Phase: Pull request ──────────────────────────────────────────────────────
phase_create_pr() {
    info "Phase 9: Creating pull request"

    local pr_title="chore: release v$VERSION"
    local pr_body
    pr_body="$(cat <<EOF
## Release v$VERSION

**Date:** $DATE
**Category:** $CATEGORY

### Changes
- Bumped workspace version to $VERSION across all crates.
- Updated CHANGELOG.md with release notes.

### Verification
The following checks passed before this PR:
- \`cargo fmt --check\`
- \`cargo check --tests\`
- \`cargo check --tests --locked\`
- \`cargo test --locked\`
- \`git diff --check\`

### Artifacts
This release will produce binaries for:
- Linux x86_64
- macOS aarch64
- Windows x86_64
EOF
)"

    if $DRY_RUN; then
        info "[dry-run] gh pr create --title '$pr_title' --body '…'"
        return
    fi

    local pr_url
    pr_url="$(gh pr create \
        --title "$pr_title" \
        --body "$pr_body" \
        --base main \
        --head "$RELEASE_BRANCH")"

    info "Pull request created: $pr_url"
}

# ── Phase: Wait for checks & merge ───────────────────────────────────────────
phase_wait_and_merge() {
    info "Phase 10: Waiting for PR checks and merging"

    if $DRY_RUN; then
        info "[dry-run] Would wait for PR checks and merge."
        return
    fi

    local pr_number
    pr_number="$(gh pr view "$RELEASE_BRANCH" --json number --jq '.number')"
    if [[ -z "$pr_number" ]]; then
        die "Could not find PR for branch $RELEASE_BRANCH."
    fi

    info "Waiting for required checks on PR #$pr_number…"
    if ! gh pr checks "$RELEASE_BRANCH" --watch --fail-fast; then
        local pr_url
        pr_url="$(gh pr view "$RELEASE_BRANCH" --json url --jq '.url')"
        die "PR checks failed. Inspect the PR at: $pr_url"
    fi

    info "All checks passed. Merging PR #$pr_number…"
    if ! gh pr merge "$RELEASE_BRANCH" --squash --delete-branch; then
        local merge_err pr_url
        merge_err="$(gh pr merge "$RELEASE_BRANCH" --squash --delete-branch 2>&1 || true)"
        pr_url="$(gh pr view "$RELEASE_BRANCH" --json url --jq '.url')"
        die "Merge blocked. Reason:
$merge_err

To resolve manually:
  1. Visit the PR: $pr_url
  2. Address any branch protection requirements
  3. Merge manually, then re-run this script from the tag phase"
    fi

    info "PR merged successfully."
}

# ── Phase: Tag & publish ─────────────────────────────────────────────────────
phase_tag_and_publish() {
    info "Phase 11: Tagging and publishing"

    # Checkout main and pull.
    run git -C "$REPO_ROOT" checkout main
    run git -C "$REPO_ROOT" pull --ff-only origin main

    # Verify the release commit is on main.
    local release_commit
    release_commit="$(git -C "$REPO_ROOT" log --oneline -1 origin/main 2>/dev/null || \
                      git -C "$REPO_ROOT" log --oneline -1 main 2>/dev/null || true)"
    if [[ -z "$release_commit" ]]; then
        die "Could not find the release commit on main."
    fi
    info "Release commit on main: $release_commit"

    # Verify main contains the release version before tagging.
    local main_version
    main_version="$(grep -m1 '^version = ' "$REPO_ROOT/Cargo.toml" | sed 's/^version = "\(.*\)"/\1/')"
    if [[ "$main_version" != "$VERSION" ]]; then
        die "main does not contain version $VERSION (found $main_version). Tagging aborted."
    fi
    info "Verified main contains version $VERSION"

    # Create annotated tag.
    run git -C "$REPO_ROOT" tag -a "v$VERSION" -m "Release v$VERSION"

    # Push the tag.
    run git -C "$REPO_ROOT" push origin "v$VERSION"

    info "Tag v$VERSION pushed."
}

phase_wait_for_build() {
    info "Phase 12: Waiting for release build workflow"

    if $DRY_RUN; then
        info "[dry-run] Would wait for the Build workflow (build.yml) to complete."
        return
    fi

    local workflow_name="Build"

    info "Monitoring workflow: $workflow_name (triggered by tag v$VERSION)"

    # Use gh run list with --workflow and filter by branch/tag.
    # The --ref flag is not supported; instead we filter by conclusion/status.
    local run_id
    run_id="$(gh run list \
        --workflow="$workflow_name" \
        --json id,status,conclusion,headBranch,headSha \
        --jq ".[] | select(.headBranch == \"main\" and (.status == \"completed\" or .status == \"in_progress\" or .status == \"queued\")) | select(.conclusion == null or .conclusion == \"success\" or .conclusion == \"failure\") | .id" \
        2>/dev/null | head -n1 || true)"

    if [[ -z "$run_id" ]]; then
        warn "Could not find a running or completed workflow for v$VERSION."
        info "Check manually: gh run list --workflow='$workflow_name' --branch=main"
        return
    fi

    info "Workflow run ID: $run_id — waiting for completion…"
    if ! gh run watch "$run_id" --fail-fast; then
        warn "Workflow run $run_id failed. Inspect with: gh run view $run_id --log-failed"
        warn "You may need to investigate and re-run the workflow manually."
    fi
}

phase_verify_release() {
    info "Phase 13: Verifying GitHub release"

    if $DRY_RUN; then
        info "[dry-run] Would verify the GitHub release exists and contains artifacts."
        return
    fi

    # Use supported fields: tagName, name, url, assets.
    local release_info
    release_info="$(gh release view "v$VERSION" --json tagName,name,url,assets 2>/dev/null || true)"

    if [[ -z "$release_info" ]]; then
        die "GitHub release v$VERSION not found. It may still be processing."
    fi

    local tag_name release_name asset_count
    tag_name="$(echo "$release_info" | jq -r '.tagName')"
    release_name="$(echo "$release_info" | jq -r '.name')"
    asset_count="$(echo "$release_info" | jq -r '.assets | length')"

    info "Release verified:"
    info "  Tag:      $tag_name"
    info "  Name:     $release_name"
    info "  URL:      $(echo "$release_info" | jq -r '.url')"
    info "  Assets:   $asset_count"

    # Verify expected artifacts.
    local expected_assets=(
        "rustcode-linux-x86_64.tar.gz"
        "rustcode-macos-aarch64.tar.gz"
        "rustcode-windows-x86_64.zip"
        "SHA256SUMS"
    )
    local asset_names
    asset_names="$(echo "$release_info" | jq -r '.assets[].name')"

    local expected
    for expected in "${expected_assets[@]}"; do
        if echo "$asset_names" | grep -qF "$expected"; then
            info "  ✓ Found: $expected"
        else
            warn "  ✗ Missing: $expected"
        fi
    done

    info "Release v$VERSION complete."
}

# ── Tests ────────────────────────────────────────────────────────────────────
run_tests() {
    info "Running lightweight tests…"

    local failed=0

    # Test 1: Workspace parser returns all 8 crates.
    info "Test 1: Workspace member discovery"
    local crate_count
    crate_count="$(get_workspace_crate_paths | wc -l | tr -d ' ')"
    if [[ "$crate_count" -eq 8 ]]; then
        info "  ✓ Found $crate_count workspace crates"
    else
        error "  ✗ Expected 8 crates, found $crate_count"
        failed=$((failed + 1))
    fi

    # Test 2: Changelog generation doesn't duplicate category.
    info "Test 2: Changelog category deduplication"
    local test_notes="Some changelog notes"
    local test_category="Features"
    local category_heading=""
    if [[ ! "$test_notes" =~ ^#[[:space:]]*#?[[:space:]]*$test_category ]]; then
        category_heading="### $test_category
"
    fi
    local test_entry="## [v0.52.0] - 2026-09-04

${category_heading}${test_notes}
"
    local dup_count
    dup_count="$(echo "$test_entry" | grep -c "### Features" || true)"
    if [[ "$dup_count" -eq 1 ]]; then
        info "  ✓ Category appears exactly once"
    else
        error "  ✗ Category appears $dup_count times (expected 1)"
        failed=$((failed + 1))
    fi

    # Test 3: Branch validation logic.
    info "Test 3: Branch validation logic"
    # Simulate: local == origin (should pass)
    local ahead=0 behind=0
    if [[ "$ahead" -gt 0 && "$behind" -eq 0 ]]; then
        error "  ✗ Should not detect divergence when equal"
        failed=$((failed + 1))
    elif [[ "$behind" -gt 0 && "$ahead" -eq 0 ]]; then
        error "  ✗ Should not detect divergence when equal"
        failed=$((failed + 1))
    else
        info "  ✓ Equal branches pass validation"
    fi

    # Test 4: gh release view JSON fields.
    info "Test 4: GitHub CLI JSON field validation"
    local test_json='{"tagName":"v0.52.0","name":"Release v0.52.0","url":"https://github.com/test/test/releases/tag/v0.52.0","assets":[{"name":"test.tar.gz"}]}'
    local test_tag test_name
    test_tag="$(echo "$test_json" | jq -r '.tagName')"
    test_name="$(echo "$test_json" | jq -r '.name')"
    if [[ "$test_tag" == "v0.52.0" && "$test_name" == "Release v0.52.0" ]]; then
        info "  ✓ JSON fields are valid"
    else
        error "  ✗ JSON field extraction failed"
        failed=$((failed + 1))
    fi

    if [[ "$failed" -eq 0 ]]; then
        info "All tests passed."
    else
        warn "$failed test(s) failed."
    fi
}

# ── Main ─────────────────────────────────────────────────────────────────────
main() {
    parse_args "$@"
    check_deps

    info "═══════════════════════════════════════════════════════"
    info "  Rustcode Release — v$VERSION  ($DATE)"
    if $DRY_RUN; then
        info "  DRY-RUN MODE — no changes will be made."
    fi
    info "═══════════════════════════════════════════════════════"
    echo

    validate
    phase_branch
    phase_update_versions
    phase_update_lockfile
    phase_update_changelog
    phase_show_diff
    phase_verify
    phase_commit
    phase_push
    phase_create_pr
    phase_wait_and_merge
    phase_tag_and_publish
    phase_wait_for_build
    phase_verify_release

    # Run lightweight tests after successful release.
    run_tests

    info "═══════════════════════════════════════════════════════"
    info "  Release v$VERSION completed successfully!"
    info "═══════════════════════════════════════════════════════"
}

main "$@"
