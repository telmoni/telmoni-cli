#!/bin/sh
# install.sh: `curl -fsSL https://raw.githubusercontent.com/telmoni/telmoni-cli/main/install.sh | sh`
#
# Installs the `telmoni` CLI for this host.
#
# Environment:
#   TELMONI_VERSION=0.0.1       that tag's assets rather than the latest release
#   TELMONI_INSTALL_DIR         override installation directory (default: /usr/local/bin or ~/.local/bin)
#   TELMONI_INSTALL_DRY_RUN=1   print each command that would change this machine, run none (also --dry-run)

set -eu

REPO="${TELMONI_REPO:-telmoni/telmoni-cli}"
BIN="telmoni"

DRY_RUN="${TELMONI_INSTALL_DRY_RUN:-}"

status() { echo ">>> $*" >&2; }
warning() { echo "WARNING: $*" >&2; }
error() { echo "ERROR: $*" >&2; exit 1; }
available() { command -v "$1" >/dev/null 2>&1; }

run() {
    if [ -n "$DRY_RUN" ]; then
        echo "+ $*" >&2
        return 0
    fi
    "$@"
}

as_admin() {
    if [ -n "$DRY_RUN" ]; then
        run "$@"
        return 0
    fi
    if [ "$(id -u)" -eq 0 ]; then
        "$@"
    elif available sudo; then
        sudo "$@"
    elif available doas; then
        doas "$@"
    else
        error "Administrator privileges required to write to $1. Re-run as root, install sudo/doas, or set TELMONI_INSTALL_DIR."
    fi
}

require() {
    available "$1" || error "Missing required dependency: '$1'. Please install it and re-run."
}

# Resolve OS and architecture to matching release artifact names.
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
    Darwin) OS="macos" ;;
    Linux)  OS="linux" ;;
    *)      error "Unsupported operating system: $OS. Telmoni CLI supports macOS and Linux." ;;
esac

case "$ARCH" in
    x86_64|amd64)   ARCH="x86_64" ;;
    arm64|aarch64)  ARCH="aarch64" ;;
    *)              error "Unsupported CPU architecture: $ARCH. Telmoni CLI supports x86_64 and arm64/aarch64." ;;
esac

TARGET="${OS}-${ARCH}"

# Target compatibility matrix
case "$TARGET" in
    macos-aarch64) ;;
    linux-x86_64)  ;;
    macos-x86_64)  error "Intel macOS (x86_64) is not a prebuilt target. Build from source: cargo install --locked telmoni-cli" ;;
    linux-aarch64) error "Linux ARM64 is not a prebuilt target yet. Build from source: cargo install --locked telmoni-cli" ;;
    *)             error "Unsupported target platform: $TARGET" ;;
esac

# Pre-flight requirements
require curl
require tar
if ! available sha256sum && ! available shasum; then
    error "Missing required dependency: 'sha256sum' or 'shasum'. Please install it and re-run."
fi

# Pick installation directory
if [ -n "${TELMONI_INSTALL_DIR:-}" ]; then
    INSTALL_DIR="$TELMONI_INSTALL_DIR"
elif [ -w "/usr/local/bin" ]; then
    INSTALL_DIR="/usr/local/bin"
elif [ "$(id -u)" -eq 0 ]; then
    INSTALL_DIR="/usr/local/bin"
else
    # Non-root user fallback
    INSTALL_DIR="$HOME/.local/bin"
fi

# Determine version
if [ -n "${TELMONI_VERSION:-}" ]; then
    TAG="v${TELMONI_VERSION#v}"
    RELEASE_URL="https://github.com/${REPO}/releases/download/${TAG}"
else
    status "Looking up latest release..."
    LATEST_JSON=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" 2>/dev/null || true)
    if [ -z "$LATEST_JSON" ]; then
        error "Could not fetch latest release info from GitHub API. You can specify a version explicitly: TELMONI_VERSION=0.0.1 sh install.sh"
    fi
    TAG=$(echo "$LATEST_JSON" | grep '"tag_name":' | head -1 | cut -d '"' -f 4)
    if [ -z "$TAG" ]; then
        error "Could not parse latest release tag from GitHub."
    fi
    RELEASE_URL="https://github.com/${REPO}/releases/download/${TAG}"
fi

ARCHIVE="telmoni-${TARGET}.tar.gz"
DOWNLOAD_URL="${RELEASE_URL}/${ARCHIVE}"
CHECKSUM_URL="${RELEASE_URL}/SHA256SUMS"

status "Installing ${BIN} ${TAG} for ${TARGET} into ${INSTALL_DIR}..."

# Setup temporary working directory with cleanup trap
TEMP_DIR=$(mktemp -d 2>/dev/null || mktemp -d -t 'telmoni-install')
cleanup() {
    rm -rf "$TEMP_DIR"
}
trap cleanup EXIT INT TERM

# Compute SHA-256 hash of a file
sha256_of() {
    if available sha256sum; then
        sha256sum "$1" | awk '{ print $1 }'
    elif available shasum; then
        shasum -a 256 "$1" | awk '{ print $1 }'
    else
        error "No sha256 tool found (checked sha256sum, shasum)."
    fi
}

verify_checksum() {
    archive_file="$1"
    archive_name="$2"
    expected=$(awk -v name="$archive_name" '$2 == name { print $1 }' "$TEMP_DIR/SHA256SUMS")
    if [ -z "$expected" ]; then
        error "Could not find checksum for ${archive_name} in SHA256SUMS."
    fi

    actual=$(sha256_of "$archive_file")

    if [ "$expected" != "$actual" ]; then
        error "Checksum verification failed for ${archive_name}!\n  Expected: ${expected}\n  Actual:   ${actual}"
    fi
    status "Checksum verified (${actual})"
}

# Download archive and SHA256SUMS
status "Downloading ${DOWNLOAD_URL}..."
run curl -fsSL -o "$TEMP_DIR/$ARCHIVE" "$DOWNLOAD_URL"

status "Downloading ${CHECKSUM_URL}..."
run curl -fsSL -o "$TEMP_DIR/SHA256SUMS" "$CHECKSUM_URL"

if [ -z "$DRY_RUN" ]; then
    verify_checksum "$TEMP_DIR/$ARCHIVE" "$ARCHIVE"
fi

status "Extracting ${ARCHIVE}..."
run tar -xzf "$TEMP_DIR/$ARCHIVE" -C "$TEMP_DIR"

# Ensure install directory exists
if [ ! -d "$INSTALL_DIR" ]; then
    status "Creating installation directory: ${INSTALL_DIR}"
    as_admin mkdir -p "$INSTALL_DIR"
fi

status "Installing binary to ${INSTALL_DIR}/${BIN}..."
if [ -w "$INSTALL_DIR" ]; then
    run mv -f "$TEMP_DIR/$BIN" "$INSTALL_DIR/$BIN"
    run chmod 0755 "$INSTALL_DIR/$BIN"
else
    as_admin mv -f "$TEMP_DIR/$BIN" "$INSTALL_DIR/$BIN"
    as_admin chmod 0755 "$INSTALL_DIR/$BIN"
fi

status "Successfully installed ${BIN} ${TAG} to ${INSTALL_DIR}/${BIN}!"

# Verify PATH
case ":${PATH}:" in
    *:"${INSTALL_DIR}":*) ;;
    *)
        echo "" >&2
        warning "${INSTALL_DIR} is not in your PATH."
        warning "Add it to your shell configuration:"
        warning "  export PATH=\"${INSTALL_DIR}:\$PATH\""
        echo "" >&2
        ;;
esac

echo "Get started with:" >&2
echo "  telmoni --help" >&2
echo "  telmoni login" >&2
