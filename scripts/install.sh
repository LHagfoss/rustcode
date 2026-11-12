#!/usr/bin/env bash
# RustCode Installer for macOS and Linux
# Usage: curl -fsSL https://rustcode.lhagfoss.com/install.sh | bash
#        curl -fsSL https://rustcode.lhagfoss.com/install.sh | bash -s -- --no-app
#
# Installs the `rustcode` CLI and the RustCode desktop app (macOS:
# /Applications/RustCode.app; Linux: an AppImage with a menu entry, or the
# .deb with --deb). Every download is checked against the release SHA256SUMS.
# Run with --help for the options; docs/install.md has the details.

set -euo pipefail

REPO="LHagfoss/rustcode"
APP_BUNDLE_ID="com.lhagfoss.rustcode.desktop"
INSTALL_CLI=1
INSTALL_APP=1
USE_DEB=0
BIN_DIR=""
# A directory with release assets and their SHA256SUMS, used instead of
# GitHub, to test the installer without a release (docs/install.md).
ARTIFACTS="${RUSTCODE_INSTALL_ARTIFACTS:-}"

usage() {
    cat <<'USAGE'
Install the RustCode CLI and desktop app from the latest GitHub release.

Options:
  --no-app        Install only the CLI.
  --no-cli        Install only the desktop app.
  --deb           Linux: install the desktop app's .deb with dpkg (asks for
                  sudo) instead of the AppImage.
  --bin-dir DIR   Install the CLI to DIR (default /usr/local/bin when
                  writable, else ~/.local/bin).
  -h, --help      Show this help.

Environment:
  RUSTCODE_INSTALL_ARTIFACTS=DIR   Install from DIR (release asset names and a
                                   SHA256SUMS) instead of downloading.
USAGE
}

while [ $# -gt 0 ]; do
    case "$1" in
        --no-app) INSTALL_APP=0 ;;
        --no-cli) INSTALL_CLI=0 ;;
        --deb) USE_DEB=1 ;;
        --bin-dir)
            [ $# -ge 2 ] || { echo "--bin-dir needs a directory" >&2; exit 2; }
            BIN_DIR="$2"
            shift
            ;;
        --bin-dir=*) BIN_DIR="${1#--bin-dir=}" ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Unknown option: $1 (see --help)" >&2; exit 2 ;;
    esac
    shift
done
if [ "$INSTALL_CLI" = 0 ] && [ "$INSTALL_APP" = 0 ]; then
    echo "Nothing to install: --no-app and --no-cli together." >&2
    exit 2
fi
COLOR_RESET="\033[0m"
COLOR_BOLD="\033[1m"
COLOR_GREEN="\033[32m"
COLOR_CYAN="\033[36m"
COLOR_YELLOW="\033[33m"
COLOR_RED="\033[31m"

info() {
    printf "${COLOR_CYAN}${COLOR_BOLD}==>${COLOR_RESET} %s\n" "$*"
}

success() {
    printf "${COLOR_GREEN}${COLOR_BOLD}==>${COLOR_RESET} %s\n" "$*"
}

warn() {
    printf "${COLOR_YELLOW}${COLOR_BOLD}Warning:${COLOR_RESET} %s\n" "$*"
}

error() {
    printf "${COLOR_RED}${COLOR_BOLD}Error:${COLOR_RESET} %s\n" "$*" >&2
    exit 1
}

terminal_supports_animation() {
    [[ -t 1 && "${TERM:-dumb}" != "dumb" ]]
}

spinner_frame() {
    case "$(( $1 % 4 ))" in
        0) printf '|' ;;
        1) printf '/' ;;
        2) printf '-' ;;
        *) printf '\\' ;;
    esac
}

run_with_spinner() {
    local message="$1"
    shift

    if ! terminal_supports_animation; then
        info "$message"
        "$@"
        return
    fi

    "$@" &
    local command_pid=$!
    local step=0
    local bar marker frame

    printf '\033[?25l'
    while kill -0 "$command_pid" 2>/dev/null; do
        frame="$(spinner_frame "$step")"
        marker=$((step % 24))
        bar=""
        for ((index = 0; index < 24; index++)); do
            if (( index == marker )); then
                bar+='>'
            else
                bar+='.'
            fi
        done
        printf "\r\033[K${COLOR_CYAN}%s${COLOR_RESET} [${COLOR_GREEN}%s${COLOR_RESET}] %s" "$frame" "$bar" "$message"
        step=$((step + 1))
        sleep 0.08
    done

    if wait "$command_pid"; then
        command_status=0
    else
        command_status=$?
    fi
    printf '\r\033[K\033[?25h'
    return "$command_status"
}

download() {
    local url="$1"
    local destination="$2"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$url" -o "$destination"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$destination" "$url"
    else
        error "Neither curl nor wget was found."
    fi
}

# A release asset by name: copied from RUSTCODE_INSTALL_ARTIFACTS, else
# downloaded from the release.
fetch_asset() {
    local name="$1"
    local destination="$2"
    if [ -n "$ARTIFACTS" ]; then
        [ -f "${ARTIFACTS}/${name}" ] || return 1
        cp "${ARTIFACTS}/${name}" "$destination"
    else
        download "https://github.com/${REPO}/releases/download/${LATEST_TAG}/${name}" "$destination"
    fi
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print tolower($1)}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print tolower($1)}'
    else
        return 1
    fi
}

# The checksum of the one well-formed SHA256SUMS entry for an asset.
manifest_checksum() {
    awk -v asset="$1" '
        {
            filename = $2
            sub(/^\*/, "", filename)
            if (filename == asset) {
                if (NF != 2) {
                    malformed = 1
                    next
                }
                count++
                checksum = tolower($1)
            }
        }
        END {
            # mawk (Debian/Ubuntu default) has no {n} intervals.
            if (malformed || count != 1 || length(checksum) != 64 || checksum ~ /[^0-9a-f]/) {
                exit 1
            }
            print checksum
        }
    ' "$2"
}

# v0.36.0 predates the published SHA256SUMS asset. Keep this exact, one-release
# migration mapping so existing installers remain verifiable until the next tag.
legacy_checksum() {
    case "$1:$2" in
        "v0.36.0:rustcode-linux-x86_64.tar.gz")
            echo "3b469813d0c144bc9a20851116f60f3e8c0b9de4bdadb4a69c6cbc294962edb2"
            ;;
        "v0.36.0:rustcode-macos-aarch64.tar.gz")
            echo "ee34d580ebe45be9d0bd53265e1dd27cb4709a30378e7234dd44c9256af46568"
            ;;
        "v0.36.0:rustcode-windows-x86_64.zip")
            echo "dea6a42383dea5f04baa36f78e373b7faf0db303c084882e3dbb3d8d5d4a3786"
            ;;
        *)
            return 1
            ;;
    esac
}

# Detect OS
OS="$(uname -s)"
case "$OS" in
    Darwin)
        TARGET_OS="macos"
        ;;
    Linux)
        TARGET_OS="linux"
        ;;
    *)
        error "Unsupported operating system: $OS. For Windows, please run install.ps1 in PowerShell."
        ;;
esac

# Detect Architecture
ARCH="$(uname -m)"
case "$ARCH" in
    x86_64|amd64)
        TARGET_ARCH="x86_64"
        ;;
    arm64|aarch64)
        TARGET_ARCH="aarch64"
        ;;
    *)
        error "Unsupported architecture: $ARCH"
        ;;
esac

# Match GitHub Release asset name
if [ "$TARGET_OS" = "macos" ]; then
    if [ "$TARGET_ARCH" = "aarch64" ]; then
        ASSET_NAME="rustcode-macos-aarch64.tar.gz"
    else
        error "Intel macOS is not distributed via prebuilt binaries. Build from source instead: cargo install --path rustcode/tui"
    fi
elif [ "$TARGET_OS" = "linux" ]; then
    # Release binaries link against glibc; musl systems (Alpine, Void musl)
    # cannot run them.
    if ldd --version 2>&1 | grep -qi musl; then
        error "Prebuilt Linux binaries need glibc; this system uses musl. Build from source instead: cargo install --path rustcode/tui"
    fi
    ASSET_NAME="rustcode-linux-${TARGET_ARCH}.tar.gz"
fi

if [ -n "$ARTIFACTS" ]; then
    [ -d "$ARTIFACTS" ] || error "RUSTCODE_INSTALL_ARTIFACTS=${ARTIFACTS} is not a directory."
    ARTIFACTS="$(cd "$ARTIFACTS" && pwd)"
    LATEST_TAG="(local artifacts in ${ARTIFACTS})"
    info "Installing from ${ARTIFACTS} instead of GitHub."
else
    info "Detecting latest release for ${TARGET_OS}-${TARGET_ARCH}..."

    # Fetch latest release tag
    LATEST_TAG=""
    if command -v curl >/dev/null 2>&1; then
        LATEST_TAG=$(curl -s "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/' || true)
        if [ -z "$LATEST_TAG" ]; then
            # Fallback to redirect URL inspection
            LATEST_TAG=$(curl -s -L -I -o /dev/null -w '%{url_effective}' "https://github.com/${REPO}/releases/latest" | sed -E 's/.*\/tag\///' || true)
        fi
    elif command -v wget >/dev/null 2>&1; then
        LATEST_TAG=$(wget -qO- "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/' || true)
    else
        error "Neither curl nor wget was found."
    fi

    if [ -z "$LATEST_TAG" ]; then
        error "Could not determine the latest release tag from GitHub."
    fi
fi

TMP_DIR="$(mktemp -d)"
cleanup() {
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

MANIFEST_NAME="SHA256SUMS"
MANIFEST_PATH="${TMP_DIR}/${MANIFEST_NAME}"
HAVE_MANIFEST=1
if ! run_with_spinner "Fetching ${MANIFEST_NAME} for ${LATEST_TAG}..." fetch_asset "$MANIFEST_NAME" "$MANIFEST_PATH"; then
    HAVE_MANIFEST=0
fi

# Fetch an asset into TMP_DIR and check it against SHA256SUMS, setting
# VERIFIED_PATH. Returns 1 when it cannot be fetched; a bad checksum exits.
VERIFIED_PATH=""
fetch_verified() {
    local name="$1"
    local path="${TMP_DIR}/${name}"
    local expected actual
    if ! run_with_spinner "Downloading ${name}..." fetch_asset "$name" "$path"; then
        return 1
    fi
    if [ "$HAVE_MANIFEST" = 1 ]; then
        expected="$(manifest_checksum "$name" "$MANIFEST_PATH")" || error "SHA256SUMS has no single valid entry for ${name}."
    elif expected="$(legacy_checksum "$LATEST_TAG" "$name")"; then
        warn "${MANIFEST_NAME} is unavailable for ${LATEST_TAG}; using the embedded official one-release migration checksum."
    else
        error "Could not download ${MANIFEST_NAME} for ${LATEST_TAG}; refusing to install an unverified archive."
    fi
    actual="$(sha256_of "$path")" || error "Neither sha256sum nor shasum was found; refusing to install an unverified archive."
    if [ "$expected" != "$actual" ]; then
        error "SHA-256 mismatch for ${name}; refusing to install it."
    fi
    VERIFIED_PATH="$path"
}

# The one asset in SHA256SUMS matching an extended regex, e.g. the desktop
# app, whose name carries the version.
manifest_asset() {
    [ "$HAVE_MANIFEST" = 1 ] || return 1
    local found
    found="$(awk '{ name = $2; sub(/^\*/, "", name); print name }' "$MANIFEST_PATH" | grep -E "$1" || true)"
    [ -n "$found" ] && [ "$(printf '%s\n' "$found" | wc -l | tr -d ' ')" = 1 ] || return 1
    printf '%s\n' "$found"
}

install_cli() {
    local archive extracted
    if ! fetch_verified "$ASSET_NAME"; then
        if [ "$ASSET_NAME" = "rustcode-linux-aarch64.tar.gz" ]; then
            error "RustCode ${LATEST_TAG} has no Linux ARM64 binary (published from v0.79.0). Build from source instead: cargo install --path rustcode/tui"
        fi
        error "Could not download ${ASSET_NAME}."
    fi
    archive="$VERIFIED_PATH"

    mkdir -p "${TMP_DIR}/cli"
    if ! run_with_spinner "Extracting archive..." tar -xzf "$archive" -C "${TMP_DIR}/cli"; then
        error "Could not extract ${ASSET_NAME}."
    fi

    extracted="$(find "${TMP_DIR}/cli" -type f -name "rustcode*" ! -name "*.tar.gz" | head -n 1)"
    if [ -z "$extracted" ]; then
        error "Extracted archive did not contain rustcode binary."
    fi

    chmod +x "$extracted"

    # Target installation directory
    if [ -n "$BIN_DIR" ]; then
        INSTALL_DIR="$BIN_DIR"
    elif [ -w "/usr/local/bin" ]; then
        INSTALL_DIR="/usr/local/bin"
    else
        INSTALL_DIR="${HOME}/.local/bin"
    fi

    mkdir -p "$INSTALL_DIR"
    TARGET_EXE="${INSTALL_DIR}/rustcode"

    if ! run_with_spinner "Installing to ${TARGET_EXE}..." cp "$extracted" "$TARGET_EXE"; then
        error "Could not install RustCode to ${TARGET_EXE}."
    fi
    chmod +x "$TARGET_EXE"

    # Keep one code-signing identity across updates when the user has set up the
    # local certificate (scripts/macos-stable-signature.sh); otherwise macOS
    # Keychain asks for the login password again after every update.
    if [ "$(uname -s)" = "Darwin" ] && command -v codesign >/dev/null 2>&1; then
        if codesign --force --sign "RustCode Local Signing" --identifier org.rustcode.cli "$TARGET_EXE" >/dev/null 2>&1; then
            success "Signed with the local \"RustCode Local Signing\" certificate."
        fi
    fi

    success "RustCode ${LATEST_TAG} installed successfully to ${TARGET_EXE}!"

    # Check PATH
    case ":$PATH:" in
        *":${INSTALL_DIR}:"*)
            ;;
        *)
            warn "${INSTALL_DIR} is not in your PATH."
            echo ""
            echo "Add it to your shell configuration file:"
            echo ""
            CURRENT_SHELL="$(basename "${SHELL:-/bin/bash}")"
            case "$CURRENT_SHELL" in
                zsh)
                    echo "    echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.zshrc"
                    echo "    source ~/.zshrc"
                    ;;
                fish)
                    echo "    fish_add_path ${INSTALL_DIR}"
                    ;;
                *)
                    echo "    echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.bashrc"
                    echo "    source ~/.bashrc"
                    ;;
            esac
            echo ""
            ;;
    esac

    # A previous Cargo or system install can appear earlier in PATH than the
    # freshly installed binary. Detect that explicitly so the user does not keep
    # invoking an older updater after a successful installation.
    RESOLVED_RUSTCODE="$(command -v rustcode 2>/dev/null || true)"
    if [ -n "$RESOLVED_RUSTCODE" ] && [ "$RESOLVED_RUSTCODE" != "$TARGET_EXE" ]; then
        warn "Your shell currently resolves rustcode to ${RESOLVED_RUSTCODE}."
        echo "Run '${TARGET_EXE} --version' now, or put '${INSTALL_DIR}' before it in PATH."
        echo "Then refresh the shell command cache with 'hash -r' or 'rehash'."
    fi
}

LSREGISTER="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"

app_is_running() {
    [ "$(osascript -e "application id \"${APP_BUNDLE_ID}\" is running" 2>/dev/null)" = "true" ]
}

# Ask a running RustCode (this app or the SwiftUI one it replaces, which share
# the bundle id) to quit, and wait for it.
quit_running_app() {
    app_is_running || return 0
    info "Quitting the running RustCode..."
    osascript -e "tell application id \"${APP_BUNDLE_ID}\" to quit" >/dev/null 2>&1 || true
    local waited=0
    while app_is_running; do
        if [ "$waited" -ge 30 ]; then
            error "RustCode is still running. Quit it, then run the installer again."
        fi
        sleep 0.5
        waited=$((waited + 1))
    done
}

# Move an app to the Trash, never delete it, so the old one can be put back.
move_to_trash() {
    local path="$1"
    if [ -x /usr/bin/trash ] && /usr/bin/trash "$path" >/dev/null 2>&1; then
        return 0
    fi
    if osascript -e "tell application \"Finder\" to delete (POSIX file \"${path}\" as alias)" >/dev/null 2>&1; then
        return 0
    fi
    local trashed
    trashed="${HOME}/.Trash/$(basename "$path" .app) $(date +%Y-%m-%d\ %H.%M.%S).app"
    mkdir -p "${HOME}/.Trash" && mv "$path" "$trashed"
}

# Sign a copy of the downloaded app with the local "RustCode Local Signing"
# certificate (scripts/macos-stable-signature.sh), the way the app's own updater
# does (apps/desktop/src-tauri/src/updater.rs `locally_signed_bundle`). An
# ad-hoc bundle's identity is the hash of that build, so Keychain asks for the
# login password again for every stored item after each install. Prints the
# bundle to install: the signed copy, or the original when there is no
# certificate or signing fails.
locally_signed_app() {
    local original="$1" copy desktop="com.lhagfoss.rustcode.desktop"
    if ! security find-certificate -c "RustCode Local Signing" >/dev/null 2>&1; then
        printf '%s\n' "$original"
        return 0
    fi
    copy="${TMP_DIR}/signed/RustCode.app"
    mkdir -p "${TMP_DIR}/signed"
    if ditto "$original" "$copy" \
        && codesign --force --sign "RustCode Local Signing" --identifier org.rustcode.cli "${copy}/Contents/MacOS/rustcode" >/dev/null 2>&1 \
        && codesign --force --sign "RustCode Local Signing" --identifier "$desktop" "${copy}/Contents/MacOS/rustcode-desktop" >/dev/null 2>&1 \
        && codesign --force --sign "RustCode Local Signing" --identifier "$desktop" "$copy" >/dev/null 2>&1 \
        && codesign --verify --deep --strict "$copy" >/dev/null 2>&1; then
        success "Signed RustCode.app with the local \"RustCode Local Signing\" certificate." >&2
        printf '%s\n' "$copy"
    else
        warn "Could not sign RustCode.app with the local certificate; installing it as downloaded." >&2
        printf '%s\n' "$original"
    fi
}

install_app_macos() {
    local name zip app id dest_dir dest
    if ! name="$(manifest_asset '^RustCode-[0-9][^ ]*-macos-aarch64\.app\.zip$')"; then
        warn "Release ${LATEST_TAG} has no desktop app for macOS; skipping it."
        return 0
    fi
    fetch_verified "$name" || error "Could not download ${name}."
    zip="$VERIFIED_PATH"

    mkdir -p "${TMP_DIR}/app"
    run_with_spinner "Unpacking ${name}..." ditto -x -k "$zip" "${TMP_DIR}/app" || error "Could not unpack ${name}."
    app="${TMP_DIR}/app/RustCode.app"
    [ -f "${app}/Contents/Info.plist" ] || error "${name} has no RustCode.app."
    id="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "${app}/Contents/Info.plist" 2>/dev/null || true)"
    [ "$id" = "$APP_BUNDLE_ID" ] || error "${name} is not the RustCode app (bundle id ${id:-missing})."
    codesign --verify --deep --strict "$app" >/dev/null 2>&1 || error "RustCode.app in ${name} has an invalid code signature."
    app="$(locally_signed_app "$app")"

    if [ -w /Applications ]; then
        dest_dir="/Applications"
    else
        dest_dir="${HOME}/Applications"
        mkdir -p "$dest_dir"
        warn "/Applications is not writable; installing to ${dest_dir}."
    fi
    dest="${dest_dir}/RustCode.app"

    quit_running_app
    if [ -e "$dest" ]; then
        info "Moving the existing ${dest} to the Trash..."
        move_to_trash "$dest" || error "Could not move ${dest} to the Trash; move it yourself and run the installer again."
    fi
    run_with_spinner "Installing ${dest}..." ditto "$app" "$dest" || error "Could not install ${dest}."
    # The checksum above vouches for this app; clear quarantine on it alone.
    xattr -dr com.apple.quarantine "$dest" 2>/dev/null || true
    [ -x "$LSREGISTER" ] && "$LSREGISTER" -f "$dest" >/dev/null 2>&1 || true

    success "RustCode.app installed to ${dest}. Open it from Launchpad, Spotlight or: open -a RustCode"
}

install_app_linux() {
    local machine name file
    machine="$TARGET_ARCH"
    if [ "$USE_DEB" = 1 ]; then
        command -v dpkg >/dev/null 2>&1 || error "--deb needs dpkg; drop it to install the AppImage."
        if ! name="$(manifest_asset "^RustCode-[0-9][^ ]*-linux-${machine}\\.deb\$")"; then
            warn "Release ${LATEST_TAG} has no desktop .deb for Linux ${machine}; skipping the app."
            return 0
        fi
        fetch_verified "$name" || error "Could not download ${name}."
        file="$VERIFIED_PATH"
        local sudo=""
        [ "$(id -u)" = 0 ] || sudo="sudo"
        info "Installing ${name} (this asks for your password)..."
        if command -v apt-get >/dev/null 2>&1; then
            $sudo apt-get install -y "$file" || error "Could not install ${name}."
        else
            $sudo dpkg -i "$file" || error "Could not install ${name}; its dependencies may be missing (libwebkit2gtk-4.1)."
        fi
        success "RustCode installed from ${name}. Start it from your applications menu or with: rustcode-desktop"
        return 0
    fi

    if ! name="$(manifest_asset "^RustCode-[0-9][^ ]*-linux-${machine}\\.AppImage\$")"; then
        warn "Release ${LATEST_TAG} has no desktop AppImage for Linux ${machine}; skipping the app."
        return 0
    fi
    fetch_verified "$name" || error "Could not download ${name}."
    file="$VERIFIED_PATH"
    chmod +x "$file"

    local apps="${HOME}/Applications"
    local data="${XDG_DATA_HOME:-${HOME}/.local/share}"
    local target="${apps}/RustCode.AppImage"
    mkdir -p "$apps" "${data}/applications" "${data}/icons/hicolor/128x128/apps"
    # Replacing the file is safe while it runs: the running copy stays mounted.
    if ! { cp "$file" "${target}.new" && mv -f "${target}.new" "$target"; }; then
        error "Could not install ${target}."
    fi

    # The icon comes out of the AppImage itself; without it the menu shows a generic one.
    local icon
    if (cd "$TMP_DIR" && "$file" --appimage-extract >/dev/null 2>&1); then
        icon="$(find "${TMP_DIR}/squashfs-root/usr/share/icons" -path '*128x128/apps/*.png' 2>/dev/null | head -n 1)"
        [ -n "$icon" ] || icon="$(find "${TMP_DIR}/squashfs-root" -maxdepth 1 -name '*.png' 2>/dev/null | head -n 1)"
        [ -n "$icon" ] && cp "$icon" "${data}/icons/hicolor/128x128/apps/rustcode.png"
    fi

    cat > "${data}/applications/rustcode.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=RustCode
Comment=The local coding agent
Exec="${target}" %U
Icon=rustcode
Terminal=false
Categories=Development;
DESKTOP
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "${data}/applications" >/dev/null 2>&1 || true

    success "RustCode installed to ${target}, with a menu entry."
    if ! command -v fusermount >/dev/null 2>&1 && ! command -v fusermount3 >/dev/null 2>&1; then
        warn "AppImages need FUSE (libfuse2) to run; install it, or rerun with --deb."
    fi
}

if [ "$INSTALL_CLI" = 1 ]; then
    install_cli
fi

if [ "$INSTALL_APP" = 1 ]; then
    if [ "$TARGET_OS" = "macos" ]; then
        install_app_macos
    else
        install_app_linux
    fi
fi

if [ "$INSTALL_CLI" = 1 ]; then
    echo "Run 'rustcode' to start pair programming!"
fi
