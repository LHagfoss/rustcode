#!/usr/bin/env bash
# RustCode Installer for macOS and Linux
# Usage: curl -fsSL https://raw.githubusercontent.com/LHagfoss/rustcode/main/install.sh | bash

set -euo pipefail

REPO="LHagfoss/rustcode"
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
        ASSET_NAME="rustcode-macos-x86_64.tar.gz"
    fi
elif [ "$TARGET_OS" = "linux" ]; then
    if [ "$TARGET_ARCH" = "x86_64" ]; then
        ASSET_NAME="rustcode-linux-x86_64.tar.gz"
    else
        error "Linux $ARCH is not yet distributed via prebuilt binaries. Please build with cargo install."
    fi
fi

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

DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${LATEST_TAG}/${ASSET_NAME}"
info "Downloading RustCode ${LATEST_TAG} (${ASSET_NAME})..."

TMP_DIR="$(mktemp -d)"
cleanup() {
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

ARCHIVE_PATH="${TMP_DIR}/${ASSET_NAME}"
download "$DOWNLOAD_URL" "$ARCHIVE_PATH"

MANIFEST_NAME="SHA256SUMS"
MANIFEST_PATH="${TMP_DIR}/${MANIFEST_NAME}"
MANIFEST_URL="https://github.com/${REPO}/releases/download/${LATEST_TAG}/${MANIFEST_NAME}"
info "Verifying ${ASSET_NAME} against ${MANIFEST_NAME}..."
if ! download "$MANIFEST_URL" "$MANIFEST_PATH"; then
    if EXPECTED_SHA256="$(legacy_checksum "$LATEST_TAG" "$ASSET_NAME")"; then
        warn "${MANIFEST_NAME} is unavailable for ${LATEST_TAG}; using the embedded official one-release migration checksum."
    else
        error "Could not download ${MANIFEST_NAME} for ${LATEST_TAG}; refusing to install an unverified archive."
    fi
else
    EXPECTED_SHA256="$(awk -v asset="$ASSET_NAME" '
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
            if (malformed || count != 1 || checksum !~ /^[0-9a-f]{64}$/) {
                exit 1
            }
            print checksum
        }
    ' "$MANIFEST_PATH")" || error "SHA256SUMS has no single valid entry for ${ASSET_NAME}."
fi

if command -v sha256sum >/dev/null 2>&1; then
    ACTUAL_SHA256="$(sha256sum "$ARCHIVE_PATH" | awk '{print tolower($1)}')"
elif command -v shasum >/dev/null 2>&1; then
    ACTUAL_SHA256="$(shasum -a 256 "$ARCHIVE_PATH" | awk '{print tolower($1)}')"
else
    error "Neither sha256sum nor shasum was found; refusing to install an unverified archive."
fi
if [ "$EXPECTED_SHA256" != "$ACTUAL_SHA256" ]; then
    error "SHA-256 mismatch for ${ASSET_NAME}; refusing to install the archive."
fi

info "Extracting archive..."
tar -xzf "$ARCHIVE_PATH" -C "$TMP_DIR"

EXTRACTED_BIN="$(find "$TMP_DIR" -type f -name "rustcode*" ! -name "*.tar.gz" | head -n 1)"
if [ -z "$EXTRACTED_BIN" ]; then
    error "Extracted archive did not contain rustcode binary."
fi

chmod +x "$EXTRACTED_BIN"

# Target installation directory
if [ -w "/usr/local/bin" ]; then
    INSTALL_DIR="/usr/local/bin"
else
    INSTALL_DIR="${HOME}/.local/bin"
fi

mkdir -p "$INSTALL_DIR"
TARGET_EXE="${INSTALL_DIR}/rustcode"

info "Installing to ${TARGET_EXE}..."
cp "$EXTRACTED_BIN" "$TARGET_EXE"
chmod +x "$TARGET_EXE"

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

echo "Run 'rustcode' to start pair programming!"
