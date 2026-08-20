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
    ASSET_NAME="rustcode-macos-aarch64.tar.gz"
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
if command -v curl >/dev/null 2>&1; then
    curl -fSL "$DOWNLOAD_URL" -o "$ARCHIVE_PATH"
elif command -v wget >/dev/null 2>&1; then
    wget -qO "$ARCHIVE_PATH" "$DOWNLOAD_URL"
else
    error "Neither curl nor wget was found."
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
