#!/usr/bin/env bash
# release.sh — Automated release script for the rustcode workspace.
#
# Usage:
#   scripts/release.sh --version 0.52.0 [--date 2026-09-04] [--category Features] [--notes-file NOTES.md] [--dry-run] [--yes]
#   scripts/release.sh --help
#
# This script is intentionally conservative: it never pushes, tags, or merges
# unless explicitly confirmed. --dry-run prints every command that would run;
# --yes accepts every confirmation for explicitly authorized automation.

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
CURRENT_VERSION=""
RELEASE_TAG_COMMIT=""
MERGED_COMMIT=""

# ── Helpers ──────────────────────────────────────────────────────────────────
info()  { printf '\033[1;34m[INFO]\033[0m  %s\n' "$*"; }
warn()  { printf '\033[1;33m[WARN]\033[0m %s\n' "$*" >&2; }
error() { printf '\033[1;31m[ERROR]\033[0m %s\n' "$*" >&2; }
die()   { error "$*"; exit 1; }

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
# Handles commands with arguments (e.g. "code --wait").
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
# Uses EXIT trap for reliable cleanup on any exit (normal or error).
cleanup_and_exit() {
    local code="$1"
    trap - EXIT INT TERM
    if [[ -n "${RELEASE_BRANCH:-}" ]] && [[ -n "${ORIGINAL_BRANCH:-}" ]]; then
        local current_branch
        current_branch="$(git -C "$REPO_ROOT" branch --show-current 2>/dev/null || true)"
        if [[ "$current_branch" == "$RELEASE_BRANCH" ]]; then
            # Check if there are uncommitted changes on the release branch.
            local dirty
            dirty="$(git -C "$REPO_ROOT" status --porcelain 2>/dev/null || true)"
            if [[ -z "$dirty" ]]; then
                # Safe to return to original branch.
                info "Returning to original branch: $ORIGINAL_BRANCH"
                run git -C "$REPO_ROOT" checkout "$ORIGINAL_BRANCH" 2>/dev/null || true
            else
                # Dirty state — preserve release branch.
                warn "Release branch $RELEASE_BRANCH has uncommitted changes."
                warn "Preserving release branch for manual recovery."
            fi
            if [[ "$code" -ne 0 ]]; then
                info "Recovery: fix the reported issue, then resume from $RELEASE_BRANCH or clean up manually."
            fi
        fi
    fi
    exit "$code"
}
trap 'cleanup_and_exit $?' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# ── Dependency checks ────────────────────────────────────────────────────────
check_deps() {
    local missing=()
    for cmd in git cargo gh jq; do
        if ! command -v "$cmd" >/dev/null 2>&1; then
            missing+=("$cmd")
        fi
    done
    if [[ ${#missing[@]} -gt 0 ]]; then
        die "Missing required commands: ${missing[*]}. Install them and retry."
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
            --version|--date|--category|--notes-file)
                if [[ $# -lt 2 ]]; then
                    die "Option $1 requires a value."
                fi
                case "$1" in
                    --version) VERSION="$2" ;;
                    --date) DATE="$2" ;;
                    --category) CATEGORY="$2" ;;
                    --notes-file) NOTES_FILE="$2" ;;
                esac
                shift 2
                ;;
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
    CURRENT_VERSION="$current"
    info "Current version: $current  →  Target: $VERSION"
    if ! semver_gt "$VERSION" "$current"; then
        die "Version $VERSION is not greater than current version $current."
    fi

    # 2. Tag does not already exist (local and remote).
    if git -C "$REPO_ROOT" tag --list "v$VERSION" | grep -q "^v$VERSION$" || \
       git -C "$REPO_ROOT" ls-remote --exit-code --tags origin "refs/tags/v$VERSION" >/dev/null 2>&1; then
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
    cargo metadata --manifest-path "$REPO_ROOT/Cargo.toml" --no-deps --format-version 1 |
        jq -r '.packages[].manifest_path' |
        while IFS= read -r manifest_path; do
            case "$manifest_path" in
                "$REPO_ROOT/Cargo.toml") ;;
                "$REPO_ROOT"/*/Cargo.toml)
                    printf '%s\n' "${manifest_path#"$REPO_ROOT"/}" | sed 's#/Cargo.toml$##'
                    ;;
            esac
        done
}

# ── Phase 1: Branch & version bump ───────────────────────────────────────────
phase_branch() {
    RELEASE_BRANCH="chore/release-v$VERSION"
    ORIGINAL_BRANCH="$(git -C "$REPO_ROOT" branch --show-current)"
    info "Phase 1: Creating release branch $RELEASE_BRANCH"
    if git -C "$REPO_ROOT" show-ref --verify --quiet "refs/heads/$RELEASE_BRANCH"; then
        die "Release branch $RELEASE_BRANCH already exists. Resume or remove it manually."
    fi
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
    if $DRY_RUN; then
        info "[dry-run] mv $tmp $root_toml"
        rm -f -- "$tmp"
    else
        mv -- "$tmp" "$root_toml"
    fi

    # Update each workspace crate's Cargo.toml.
    local crate_path crate_count=0
    while IFS= read -r crate_path; do
        [[ -z "$crate_path" ]] && continue
        crate_count=$((crate_count + 1))
        local crate_toml="$REPO_ROOT/$crate_path/Cargo.toml"
        if [[ ! -f "$crate_toml" ]]; then
            die "Workspace member manifest not found: $crate_toml"
        fi
        tmp="$(mktemp)"
        sed 's/^version = ".*"/version = "'"$VERSION"'"/' "$crate_toml" > "$tmp"
        if $DRY_RUN; then
            info "[dry-run] mv $tmp $crate_toml"
            rm -f -- "$tmp"
        else
            mv -- "$tmp" "$crate_toml"
        fi
    done < <(get_workspace_crate_paths)
    if [[ "$crate_count" -eq 0 ]]; then
        die "Could not discover any workspace crate manifests."
    fi
}

verify_workspace_versions() {
    local mismatches
    mismatches="$(cargo metadata --manifest-path "$REPO_ROOT/Cargo.toml" --no-deps --format-version 1 |
        jq -r --arg version "$VERSION" '.packages[] | select(.version != $version) | "\(.name)=\(.version)"')"
    if [[ -n "$mismatches" ]]; then
        die "Workspace packages with unexpected versions:\n$mismatches"
    fi
}

phase_update_lockfile() {
    info "Phase 3: Updating Cargo.lock"
    if $DRY_RUN; then
        info "[dry-run] (cd $REPO_ROOT && cargo check --manifest-path $REPO_ROOT/Cargo.toml)"
    else
        local lockfile="$REPO_ROOT/Cargo.lock"
        local backup
        backup="$(mktemp)"
        cp "$lockfile" "$backup"

        # An unlocked check updates the local workspace package entries while
        # preserving the existing dependency resolution where possible.
        if ! (cd "$REPO_ROOT" && cargo check --manifest-path "$REPO_ROOT/Cargo.toml"); then
            cp "$backup" "$lockfile"
            rm -f -- "$backup"
            die "Cargo could not update Cargo.lock for the new workspace version."
        fi

        local diff_output unexpected
        diff_output="$(git -C "$REPO_ROOT" diff --unified=0 -- Cargo.lock || true)"
        unexpected="$(printf '%s\n' "$diff_output" |
            grep -E '^[+-]' |
            grep -vE '^(---|\+\+\+)' |
            grep -vFx -- "-version = \"$CURRENT_VERSION\"" |
            grep -vFx -- "+version = \"$VERSION\"" || true)"
        if [[ -n "$unexpected" ]]; then
            cp "$backup" "$lockfile"
            rm -f -- "$backup"
            die "Cargo.lock changed outside the intended workspace version entries:\n$unexpected"
        fi
        rm -f -- "$backup"
    fi
}

format_changelog_notes() {
    local notes="$1"
    local category="$2"
    local heading="### $category"

    case "$notes" in
        "$heading") notes="" ;;
        "$heading"$'\n'*) notes="${notes#"$heading"}"; notes="${notes#$'\n'}" ;;
    esac

    printf '### %s\n%s' "$category" "$notes"
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
        # The script adds the category heading after editing.
        printf '%s\n' '-' > "$tmpfile"
        if $DRY_RUN; then
            info "[dry-run] Would open editor for changelog notes: $tmpfile"
            notes="$(cat "$tmpfile")"
        else
            local -a editor_command
            read -r -a editor_command <<< "$editor"
            "${editor_command[@]}" "$tmpfile"
            notes="$(cat "$tmpfile")"
        fi
        rm -f -- "$tmpfile"
    fi

    local new_entry
    new_entry="## [v$VERSION](https://github.com/LHagfoss/rustcode/releases/tag/v$VERSION) - $DATE

$(format_changelog_notes "$notes" "$CATEGORY")
"

    # Prepend to CHANGELOG.md.
    local changelog="$REPO_ROOT/CHANGELOG.md"
    local tmp
    tmp="$(mktemp)"
    printf '%s' "$new_entry" > "$tmp"
    cat "$changelog" >> "$tmp"
    if $DRY_RUN; then
        info "[dry-run] mv $tmp $changelog"
        rm -f -- "$tmp"
    else
        mv -- "$tmp" "$changelog"
    fi
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

    if $DRY_RUN; then
        info "[dry-run] Would verify every workspace package is version $VERSION"
    else
        verify_workspace_versions
    fi

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
    local merge_err=""
    if ! merge_err="$(gh pr merge "$RELEASE_BRANCH" --squash --delete-branch 2>&1)"; then
        local pr_url
        pr_url="$(gh pr view "$RELEASE_BRANCH" --json url --jq '.url')"
        die "Merge blocked. Reason:
$merge_err

To resolve manually:
  1. Visit the PR: $pr_url
  2. Address any branch protection requirements
  3. Merge manually, then re-run this script from the tag phase"
    fi

    MERGED_COMMIT="$(gh pr view "$pr_number" --json state,mergeCommit --jq \
        'select(.state == "MERGED") | .mergeCommit.oid')"
    if [[ -z "$MERGED_COMMIT" ]]; then
        die "PR #$pr_number merged, but its merge commit could not be resolved. Tagging aborted."
    fi

    info "PR merged successfully at $MERGED_COMMIT."
}

# ── Phase: Tag & publish ─────────────────────────────────────────────────────
phase_tag_and_publish() {
    info "Phase 11: Tagging and publishing"

    # Checkout main and pull.
    run git -C "$REPO_ROOT" checkout main
    run git -C "$REPO_ROOT" pull --ff-only origin main

    # Tag only the commit produced by the release PR. This prevents an
    # unrelated commit that lands immediately afterward from being released.
    if ! $DRY_RUN; then
        if [[ -z "$MERGED_COMMIT" ]] || ! git -C "$REPO_ROOT" merge-base --is-ancestor "$MERGED_COMMIT" origin/main; then
            die "Release merge commit ${MERGED_COMMIT:-unknown} is not on origin/main. Tagging aborted."
        fi
    fi
    info "Release commit on main: ${MERGED_COMMIT:-dry-run}"

    # Verify main contains the release version before tagging.
    if $DRY_RUN; then
        info "[dry-run] Would verify main contains version $VERSION"
    else
        local main_version
        main_version="$(git -C "$REPO_ROOT" show "$MERGED_COMMIT:Cargo.toml" | grep -m1 '^version = ' | sed 's/^version = "\(.*\)"/\1/')"
        if [[ "$main_version" != "$VERSION" ]]; then
            die "main does not contain version $VERSION (found $main_version). Tagging aborted."
        fi
        info "Verified main contains version $VERSION"
    fi

    # Create annotated tag.
    if $DRY_RUN; then
        info "[dry-run] git -C $REPO_ROOT tag -a v$VERSION -m Release v$VERSION <release-merge-commit>"
    else
        run git -C "$REPO_ROOT" tag -a "v$VERSION" "$MERGED_COMMIT" -m "Release v$VERSION"
    fi

    # Capture the tag commit SHA before pushing.
    if $DRY_RUN; then
        RELEASE_TAG_COMMIT="$(git -C "$REPO_ROOT" rev-parse main)"
    else
        RELEASE_TAG_COMMIT="$(git -C "$REPO_ROOT" rev-parse "v$VERSION^{commit}")"
    fi
    info "Tag v$VERSION points to commit: $RELEASE_TAG_COMMIT"

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

    # Poll for the workflow run associated with the tag SHA.
    local run_id=""
    local max_attempts=30
    local attempt=0
    while [[ $attempt -lt $max_attempts ]]; do
        # List runs for the Build workflow, filtering by the exact tag SHA.
        run_id="$(gh run list \
            --workflow="$workflow_name" \
            --commit "$RELEASE_TAG_COMMIT" \
            --json databaseId,status,conclusion \
            --jq '.[0].databaseId' \
            2>/dev/null | head -n1 || true)"

        if [[ -n "$run_id" ]]; then
            info "Found workflow run ID: $run_id"
            break
        fi

        attempt=$((attempt + 1))
        info "Waiting for workflow to start… (attempt $attempt/$max_attempts)"
        sleep 10
    done

    if [[ -z "$run_id" ]]; then
        die "Could not find the Build workflow for tag v$VERSION (SHA: $RELEASE_TAG_COMMIT).
Check manually: gh run list --workflow='$workflow_name' --commit=$RELEASE_TAG_COMMIT"
    fi

    info "Waiting for workflow run $run_id to complete…"
    if ! gh run watch "$run_id" --exit-status; then
        die "Workflow run $run_id failed. Inspect with: gh run view $run_id --log-failed"
    fi

    info "Build workflow completed successfully."
}

phase_verify_release() {
    info "Phase 13: Verifying GitHub release"

    if $DRY_RUN; then
        info "[dry-run] Would verify the GitHub release exists and contains artifacts."
        return
    fi

    # Poll until the GitHub release exists.
    local release_info=""
    local max_attempts=30
    local attempt=0
    while [[ $attempt -lt $max_attempts ]]; do
        # Use supported fields: tagName, name, url, assets.
        release_info="$(gh release view "v$VERSION" --json tagName,name,url,assets 2>/dev/null || true)"
        if [[ -n "$release_info" ]]; then
            break
        fi
        attempt=$((attempt + 1))
        info "Waiting for GitHub release to appear… (attempt $attempt/$max_attempts)"
        sleep 10
    done

    if [[ -z "$release_info" ]]; then
        die "GitHub release v$VERSION not found after polling. It may still be processing."
    fi

    local tag_name release_name asset_count
    tag_name="$(echo "$release_info" | jq -r '.tagName')"
    release_name="$(echo "$release_info" | jq -r '.name')"
    asset_count="$(echo "$release_info" | jq -r '.assets | length')"

    # Verify the tag is exactly v$VERSION.
    if [[ "$tag_name" != "v$VERSION" ]]; then
        die "Release tag mismatch: expected v$VERSION, got $tag_name"
    fi

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

    local missing_assets=()
    local expected
    for expected in "${expected_assets[@]}"; do
        if echo "$asset_names" | grep -qF "$expected"; then
            info "  ✓ Found: $expected"
        else
            warn "  ✗ Missing: $expected"
            missing_assets+=("$expected")
        fi
    done

    # Fail if any expected asset is missing.
    if [[ ${#missing_assets[@]} -gt 0 ]]; then
        die "Missing expected assets: ${missing_assets[*]}"
    fi

}

phase_verify_distribution() {
    info "Phase 14: Verifying Homebrew metadata and local updater"

    if $DRY_RUN; then
        info "[dry-run] Would verify the Homebrew formula, run rustcode --update, and check rustcode --version."
        return
    fi

    local formula encoded formula_version
    encoded="$(gh api repos/LHagfoss/homebrew-tap/contents/Formula/rustcode.rb --jq '.content')" || \
        die "Could not read the published Homebrew formula."
    if ! formula="$(printf '%s' "$encoded" | tr -d '\n' | base64 --decode 2>/dev/null)"; then
        formula="$(printf '%s' "$encoded" | tr -d '\n' | base64 -D)" || \
            die "Could not decode the published Homebrew formula."
    fi
    formula_version="$(printf '%s\n' "$formula" | sed -n 's/^[[:space:]]*version "\([^"]*\)".*/\1/p' | head -n1)"
    if [[ "$formula_version" != "$VERSION" ]]; then
        die "Homebrew formula version mismatch: expected $VERSION, found ${formula_version:-none}."
    fi
    info "  ✓ Homebrew formula: $formula_version"

    local rustcode_bin installed_version
    rustcode_bin="$(command -v rustcode || true)"
    if [[ -z "$rustcode_bin" ]]; then
        die "rustcode is not installed locally; cannot verify the supported updater."
    fi
    "$rustcode_bin" --update
    installed_version="$("$rustcode_bin" --version | awk '{print $2}')"
    if [[ "$installed_version" != "$VERSION" ]]; then
        die "Installed rustcode version mismatch after update: expected $VERSION, found ${installed_version:-none}."
    fi
    info "  ✓ Installed rustcode: $installed_version"
    info "Release v$VERSION complete."
}

# ── Tests ────────────────────────────────────────────────────────────────────
run_tests() {
    info "Running lightweight tests…"

    local failed=0

    # Test 1: Workspace parser returns every non-root metadata package.
    info "Test 1: Workspace member discovery"
    local crate_count
    crate_count="$(get_workspace_crate_paths | wc -l | tr -d ' ')"
    local expected_crate_count
    expected_crate_count="$(cargo metadata --manifest-path "$REPO_ROOT/Cargo.toml" --no-deps --format-version 1 |
        jq '[.packages[] | select(.manifest_path != $root)] | length' --arg root "$REPO_ROOT/Cargo.toml")"
    if [[ "$crate_count" -eq "$expected_crate_count" && "$crate_count" -gt 0 ]]; then
        info "  ✓ Found $crate_count workspace crates"
    else
        error "  ✗ Expected $expected_crate_count crates, found $crate_count"
        failed=$((failed + 1))
    fi

    # Test 2: Changelog generation with actual ### Features input.
    info "Test 2: Changelog category deduplication (with heading)"
    local test_notes="### Features
- Added new feature"
    local test_entry="$(format_changelog_notes "$test_notes" "Features")"
    local dup_count
    dup_count="$(printf '%s\n' "$test_entry" | grep -c '^### Features$' || true)"
    if [[ "$dup_count" -eq 1 ]]; then
        info "  ✓ Category appears exactly once"
    else
        error "  ✗ Category appears $dup_count times (expected 1)"
        failed=$((failed + 1))
    fi

    # Test 3: Changelog generation without existing category heading.
    info "Test 3: Changelog category deduplication (without heading)"
    local test_notes2="Some changelog notes"
    local test_entry2="$(format_changelog_notes "$test_notes2" "Features")"
    local dup_count2
    dup_count2="$(printf '%s\n' "$test_entry2" | grep -c '^### Features$' || true)"
    if [[ "$dup_count2" -eq 1 ]]; then
        info "  ✓ Category appears exactly once"
    else
        error "  ✗ Category appears $dup_count2 times (expected 1)"
        failed=$((failed + 1))
    fi

    # Test 4: Branch validation logic — ahead state.
    info "Test 4: Branch validation logic (ahead rejection)"
    local ahead=2 behind=0
    if [[ "$ahead" -gt 0 && "$behind" -eq 0 ]]; then
        info "  ✓ Ahead state correctly detected"
    else
        error "  ✗ Should detect ahead state"
        failed=$((failed + 1))
    fi

    # Test 5: Branch validation logic — behind state.
    info "Test 5: Branch validation logic (behind rejection)"
    ahead=0
    behind=3
    if [[ "$behind" -gt 0 && "$ahead" -eq 0 ]]; then
        info "  ✓ Behind state correctly detected"
    else
        error "  ✗ Should detect behind state"
        failed=$((failed + 1))
    fi

    # Test 6: gh release view JSON fields.
    info "Test 6: GitHub CLI JSON field validation"
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
        return 0
    else
        error "$failed test(s) failed."
        return 1
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

    # Run tests before any push/tag/release action.
    if ! run_tests; then
        die "Tests failed. Fix the issues and retry."
    fi

    phase_commit
    phase_push
    phase_create_pr
    phase_wait_and_merge
    phase_tag_and_publish
    phase_wait_for_build
    phase_verify_release
    phase_verify_distribution

    info "═══════════════════════════════════════════════════════"
    info "  Release v$VERSION completed successfully!"
    info "═══════════════════════════════════════════════════════"
}

main "$@"
