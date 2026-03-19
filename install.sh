#!/usr/bin/env bash
set -euo pipefail

# cue installer
# Usage: curl -sSL https://raw.githubusercontent.com/rohan/cue/main/install.sh | sh
# Or:    ./install.sh [--build]

BINARY_NAME="cue"
INSTALL_DIR="${HOME}/.local/bin"
DATA_DIR="${XDG_DATA_HOME:-${HOME}/.local/share}/cue"
CONFIG_DIR="${XDG_CONFIG_HOME:-${HOME}/.config}/cue"
MODELS_DIR="${DATA_DIR}/models"
REPO="rohan/cue"
VERSION="${CUE_VERSION:-latest}"
WHISPER_MODEL="${CUE_WHISPER_MODEL:-tiny}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

info()    { echo -e "${BLUE}[cue]${NC} $*"; }
success() { echo -e "${GREEN}[cue]${NC} $*"; }
warn()    { echo -e "${YELLOW}[cue]${NC} $*"; }
error()   { echo -e "${RED}[cue]${NC} $*" >&2; }

# Parse arguments
BUILD_FROM_SOURCE=false
SKIP_MODEL=false
for arg in "$@"; do
    case "$arg" in
        --build) BUILD_FROM_SOURCE=true ;;
        --skip-model) SKIP_MODEL=true ;;
        --help|-h)
            echo "Usage: install.sh [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --build        Build from source (requires Rust toolchain)"
            echo "  --skip-model   Skip downloading the Whisper model"
            echo "  --help         Show this help"
            exit 0
            ;;
    esac
done

# Detect architecture
ARCH=$(uname -m)
OS=$(uname -s | tr '[:upper:]' '[:lower:]')

case "$ARCH" in
    x86_64)   ARCH_NAME="x86_64" ;;
    aarch64)  ARCH_NAME="aarch64" ;;
    arm64)    ARCH_NAME="aarch64" ;;
    *)
        warn "Unknown architecture: $ARCH. Will build from source."
        BUILD_FROM_SOURCE=true
        ;;
esac

info "Installing cue — real-time AI meeting companion"
info "Architecture: ${ARCH} | OS: ${OS}"
echo ""

# Create directories
info "Creating directories..."
mkdir -p "${INSTALL_DIR}"
mkdir -p "${DATA_DIR}"
mkdir -p "${CONFIG_DIR}"
mkdir -p "${MODELS_DIR}"

# Install binary
if [ "$BUILD_FROM_SOURCE" = true ]; then
    info "Building from source..."

    if ! command -v cargo &> /dev/null; then
        error "Rust toolchain not found. Install from https://rustup.rs/"
        exit 1
    fi

    # Check if we're in the cue repo directory
    if [ -f "Cargo.toml" ] && grep -q 'name = "cue"' crates/cue-cli/Cargo.toml 2>/dev/null; then
        info "Building in current directory..."
        cargo build --release
        cp "target/release/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
    else
        info "Cloning repository..."
        TMPDIR=$(mktemp -d)
        trap "rm -rf ${TMPDIR}" EXIT
        git clone "https://github.com/${REPO}.git" "${TMPDIR}/cue"
        cd "${TMPDIR}/cue"
        cargo build --release
        cp "target/release/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
    fi
else
    # Download prebuilt binary
    if [ "${VERSION}" = "latest" ]; then
        RELEASE_URL="https://github.com/${REPO}/releases/latest/download"
    else
        RELEASE_URL="https://github.com/${REPO}/releases/download/${VERSION}"
    fi

    BINARY_URL="${RELEASE_URL}/cue-${ARCH_NAME}-${OS}"

    info "Downloading binary from ${BINARY_URL}..."

    if command -v curl &> /dev/null; then
        curl -fsSL "${BINARY_URL}" -o "${INSTALL_DIR}/${BINARY_NAME}"
    elif command -v wget &> /dev/null; then
        wget -q "${BINARY_URL}" -O "${INSTALL_DIR}/${BINARY_NAME}"
    else
        error "Neither curl nor wget found. Install one and try again."
        exit 1
    fi

    chmod +x "${INSTALL_DIR}/${BINARY_NAME}"
fi

success "Binary installed to ${INSTALL_DIR}/${BINARY_NAME}"

# Download Whisper model
if [ "$SKIP_MODEL" = false ]; then
    MODEL_FILE="${MODELS_DIR}/ggml-${WHISPER_MODEL}.bin"

    if [ -f "${MODEL_FILE}" ]; then
        info "Whisper model already downloaded: ${MODEL_FILE}"
    else
        MODEL_URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-${WHISPER_MODEL}.bin"
        info "Downloading Whisper ${WHISPER_MODEL} model (~75MB)..."

        if command -v curl &> /dev/null; then
            curl -fL --progress-bar "${MODEL_URL}" -o "${MODEL_FILE}"
        elif command -v wget &> /dev/null; then
            wget --progress=bar "${MODEL_URL}" -O "${MODEL_FILE}"
        else
            warn "Could not download Whisper model. Run manually:"
            warn "  cue models download ${WHISPER_MODEL}"
        fi

        if [ -f "${MODEL_FILE}" ]; then
            success "Whisper model downloaded to ${MODEL_FILE}"
        fi
    fi
fi

# Check PATH
if ! echo "${PATH}" | grep -q "${INSTALL_DIR}"; then
    warn "${INSTALL_DIR} is not in your PATH."
    warn "Add this to your shell config (~/.bashrc or ~/.zshrc):"
    warn "  export PATH=\"\${HOME}/.local/bin:\${PATH}\""
fi

# Print success
echo ""
echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
success "cue installed successfully!"
echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "Quick start:"
echo ""
echo "  1. Set your API key:"
echo "     export ANTHROPIC_API_KEY=sk-ant-..."
echo "     (or: echo 'ANTHROPIC_API_KEY=sk-ant-...' > ~/.config/cue/.env)"
echo ""
echo "  2. Start a session:"
echo "     cue start"
echo "     cue start --prompt coding"
echo ""
echo "  3. Ask a question:"
echo "     cue ask 'What is a rate limiter?'"
echo ""
echo "Documentation: https://github.com/${REPO}"
echo ""
