#!/usr/bin/env bash
# Measure Cargo rebuild boundaries without changing tracked source.
set -uo pipefail

usage() {
    cat <<'EOF'
Usage: scripts/bench-build-boundaries.sh [options]

Measure clean, warm, and focused-edit Cargo checks for the workspace and its
packages. Results are TSV, written to stdout unless --output is supplied.

Options:
  --package NAME       Benchmark one package (repeatable; default: all)
  --output FILE        Write TSV results to FILE instead of stdout
  --target-dir DIR     Reuse this target directory instead of a temporary one
  --keep-target        Keep script-created temporary target directories
  --all-targets        Pass --all-targets to Cargo checks
  --skip-focused       Skip temporary source-mtime focused-edit checks
  --allow-cargo-clean  Permit cargo clean only for an explicit /tmp target
  --help               Show this help

Normal runs do not call cargo clean. A clean measurement starts in a fresh
temporary CARGO_TARGET_DIR. --allow-cargo-clean is an explicit opt-in for a
caller-provided disposable /tmp target and is rejected for repository targets.
EOF
}

die() {
    echo "bench-build-boundaries: $*" >&2
    exit 2
}

repo_root="$(cd "$(dirname "$0")/.." && pwd -P)"
manifest_path="$repo_root/Cargo.toml"
output_path=""
target_dir_arg=""
keep_target=0
all_targets=0
skip_focused=0
allow_cargo_clean=0
requested_packages=()

while (($# > 0)); do
    case "$1" in
        --package)
            (($# >= 2)) || die "--package requires a name"
            requested_packages+=("$2")
            shift 2
            ;;
        --output)
            (($# >= 2)) || die "--output requires a path"
            output_path="$2"
            shift 2
            ;;
        --target-dir)
            (($# >= 2)) || die "--target-dir requires a path"
            target_dir_arg="$2"
            shift 2
            ;;
        --keep-target) keep_target=1; shift ;;
        --all-targets) all_targets=1; shift ;;
        --skip-focused) skip_focused=1; shift ;;
        --allow-cargo-clean) allow_cargo_clean=1; shift ;;
        --help|-h) usage; exit 0 ;;
        *) die "unknown option: $1 (try --help)" ;;
    esac
done

command -v cargo >/dev/null 2>&1 || die "cargo is required"
command -v jq >/dev/null 2>&1 || die "jq is required for package discovery"

temp_root=""
if [[ -n "$target_dir_arg" ]]; then
    mkdir -p "$target_dir_arg" || die "cannot create target directory: $target_dir_arg"
    canonical_target_dir="$(cd "$target_dir_arg" 2>/dev/null && pwd -P)" ||
        die "cannot resolve target directory: $target_dir_arg"
    repo_real="$(cd "$repo_root" 2>/dev/null && pwd -P)" ||
        die "cannot resolve repository directory"
    repo_target="$repo_real/target"

    if ((allow_cargo_clean)); then
        canonical_tmp="$(cd /tmp 2>/dev/null && pwd -P)" ||
            die "cannot resolve /tmp"
        canonical_private_tmp=""
        if [[ -d /private/tmp ]]; then
            canonical_private_tmp="$(cd /private/tmp 2>/dev/null && pwd -P)" ||
                die "cannot resolve /private/tmp"
        fi
        if [[ "$canonical_target_dir" == "$repo_real" ||
            "$canonical_target_dir" == "$repo_target" ]]; then
            die "--allow-cargo-clean refuses the repository or its shared target directory"
        fi
        if [[ "$canonical_target_dir" == "$canonical_tmp" ||
            ( -n "$canonical_private_tmp" &&
                "$canonical_target_dir" == "$canonical_private_tmp" ) ]]; then
            die "--allow-cargo-clean refuses a temporary root itself"
        fi
        under_temp_root=0
        case "$canonical_target_dir" in
            "$canonical_tmp"/*) under_temp_root=1 ;;
        esac
        if [[ -n "$canonical_private_tmp" ]]; then
            case "$canonical_target_dir" in
                "$canonical_private_tmp"/*) under_temp_root=1 ;;
            esac
        fi
        ((under_temp_root)) ||
            die "--allow-cargo-clean only accepts a target under /tmp or /private/tmp"
    fi
    base_target="$canonical_target_dir"
else
    tmp_base="$(printenv TMPDIR 2>/dev/null || true)"
    [[ -n "$tmp_base" ]] || tmp_base=/tmp
    temp_root="$(mktemp -d "$tmp_base/rustcode-build-boundaries.XXXXXX")" || exit 1
    base_target="$temp_root/workspace"
    mkdir -p "$base_target" || exit 1
fi

created_targets=()
active_touch_file=""
active_touch_reference=""
failed_cases=0

cleanup() {
    local status=$?
    if [[ -n "$active_touch_file" && -n "$active_touch_reference" && -e "$active_touch_reference" ]]; then
        touch -r "$active_touch_reference" "$active_touch_file" 2>/dev/null || true
    fi
    if [[ "$keep_target" != 1 ]]; then
        if ((${#created_targets[@]} > 0)); then
            for target in "${created_targets[@]}"; do
                [[ -n "$target" && -d "$target" ]] && rm -rf -- "$target"
            done
        fi
        if [[ -n "$temp_root" && -d "$temp_root" ]]; then
            rm -rf -- "$temp_root"
        fi
    fi
    exit "$status"
}
trap cleanup EXIT INT TERM

if [[ -n "$output_path" && "$output_path" != "-" ]]; then
    mkdir -p "$(dirname "$output_path")" || die "cannot create output directory"
    exec 3>"$output_path" || die "cannot open output file: $output_path"
else
    exec 3>&1
fi
printf 'package\tscenario\tstatus\telapsed_ms\tcommand\tlog\n' >&3

now_ms() {
    local value
    value="$(date +%s%3N 2>/dev/null)"
    if [[ "$value" =~ ^[0-9]+$ ]]; then
        printf '%s\n' "$value"
    else
        printf '%s000\n' "$(date +%s)"
    fi
}

json_packages="$(cargo metadata --manifest-path "$manifest_path" --no-deps --format-version 1 2>/dev/null)" \
    || die "cargo metadata failed"
package_rows=()
while IFS= read -r row; do
    package_rows+=("$row")
done < <(
    jq -r '.packages[] | [.name, (.manifest_path | sub("/Cargo.toml$"; ""))] | @tsv' <<<"$json_packages"
)
((${#package_rows[@]} > 0)) || die "workspace has no packages"

packages=()
source_files=()
for row in "${package_rows[@]}"; do
    IFS=$'\t' read -r package manifest_dir <<<"$row"
    selected=1
    if ((${#requested_packages[@]} > 0)); then
        selected=0
        for requested in "${requested_packages[@]}"; do
            [[ "$requested" == "$package" ]] && selected=1
        done
    fi
    ((selected)) || continue
    packages+=("$package")
    if [[ -f "$manifest_dir/src/lib.rs" ]]; then
        source_files+=("$manifest_dir/src/lib.rs")
    elif [[ -f "$manifest_dir/src/main.rs" ]]; then
        source_files+=("$manifest_dir/src/main.rs")
    else
        source_files+=("")
    fi
done
((${#packages[@]} > 0)) || die "none of the requested packages were found"

if ((${#requested_packages[@]} > 0)); then
    for requested in "${requested_packages[@]}"; do
        found=0
        for package in "${packages[@]}"; do
            [[ "$package" == "$requested" ]] && found=1
        done
        ((found)) || die "package not found in workspace: $requested"
    done
fi

case_target() {
    local label="$1"
    local target
    if [[ -n "$target_dir_arg" ]]; then
        target="$base_target/$label"
    else
        target="$temp_root/$label"
        created_targets+=("$target")
    fi
    mkdir -p "$target"
    printf '%s\n' "$target"
}

run_case() {
    local package="$1"
    local scenario="$2"
    local target="$3"
    shift 3
    local -a args=("$@")
    local label="$package.$scenario"
    local log_dir="$target/logs"
    local log="$log_dir/$label.log"
    local start end elapsed status
    mkdir -p "$log_dir"
    start="$(now_ms)"
    CARGO_TARGET_DIR="$target" cargo "${args[@]}" >"$log" 2>&1
    status=$?
    end="$(now_ms)"
    elapsed=$((end - start))
    local status_name=pass
    if ((status != 0)); then
        status_name="fail:$status"
        failed_cases=$((failed_cases + 1))
    fi
    local command_text="cargo"
    local quoted_arg
    local log_reference="$log"
    if [[ -z "$target_dir_arg" && "$keep_target" != 1 ]]; then
        log_reference="(temporary log deleted; use --keep-target)"
    fi
    for arg in "${args[@]}"; do
        printf -v quoted_arg ' %q' "$arg"
        command_text+="$quoted_arg"
    done
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$package" "$scenario" "$status_name" "$elapsed" "$command_text" "$log_reference" >&3
    printf '[%s] %-28s %8sms (%s)\n' "$status_name" "$package/$scenario" "$elapsed" "$log" >&2
}

check_args=(check --locked)
((all_targets)) && check_args+=(--all-targets)

workspace_target="$(case_target workspace)"
if ((allow_cargo_clean)); then
    cargo clean --manifest-path "$manifest_path" --target-dir "$workspace_target" >/dev/null 2>&1 ||
        die "cargo clean failed for workspace target"
fi
run_case workspace clean "$workspace_target" "${check_args[@]}" --workspace
run_case workspace warm "$workspace_target" "${check_args[@]}" --workspace

for index in "${!packages[@]}"; do
    package="${packages[$index]}"
    package_target="$(case_target "package-$package")"
    if ((allow_cargo_clean)); then
        cargo clean --manifest-path "$manifest_path" --target-dir "$package_target" >/dev/null 2>&1 ||
            die "cargo clean failed for package target: $package"
    fi
    run_case "$package" clean "$package_target" "${check_args[@]}" --package "$package"
    run_case "$package" warm "$package_target" "${check_args[@]}" --package "$package"

    if ((skip_focused)); then continue; fi
    source_file="${source_files[$index]}"
    if [[ -z "$source_file" ]]; then
        printf '%s\tfocused-edit\tskipped\t0\t(none)\t(no src/lib.rs or src/main.rs)\n' "$package" >&3
        continue
    fi
    reference="$package_target/.mtime-reference"
    touch -r "$source_file" "$reference" || die "cannot snapshot mtime: $source_file"
    active_touch_file="$source_file"
    active_touch_reference="$reference"
    touch "$source_file" || die "cannot touch source file: $source_file"
    run_case "$package" focused-edit "$workspace_target" "${check_args[@]}" --package "$package"
    touch -r "$reference" "$source_file" || die "cannot restore mtime: $source_file"
    rm -f -- "$reference"
    active_touch_file=""
    active_touch_reference=""
done

if ((failed_cases > 0)); then
    echo "bench-build-boundaries: $failed_cases case(s) failed; see TSV logs" >&2
    exit 1
fi
