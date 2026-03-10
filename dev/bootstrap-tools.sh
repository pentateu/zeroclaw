#!/usr/bin/env bash
set -euo pipefail

# Idempotent bootstrap for local transcription tools and their runtime deps.
# Installs: snapd (if needed), whisper.cpp (snap), ffmpeg, yt-dlp.
# Downloads default ggml model into $ZEROCLAW_WHISPER_MODELS_DIR (or $HOME/.zeroclaw/models).
# Usage: sudo ./dev/bootstrap-tools.sh

if [ "$(id -u)" -ne 0 ]; then
    echo "This script requires sudo/root. Re-run with sudo." >&2
    exit 1
fi

# Ensure common locations are on PATH so installs (pipx, pip --user, snap) are discoverable
# Include /snap/bin, common local bin locations for root and invoking sudo user.
SUDO_INVOKER=${SUDO_USER:-}
export PATH="/snap/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/root/.local/bin:${HOME}/.local/bin${SUDO_INVOKER:+:/home/$SUDO_INVOKER/.local/bin}:$PATH"

cmd_exists() {
    # Check PATH first
    if command -v "$1" >/dev/null 2>&1; then
        return 0
    fi
    # Also check common non-PATH locations that package installers may use
    [ -x "/root/.local/bin/$1" ] && return 0
    [ -x "$HOME/.local/bin/$1" ] && return 0
    [ -n "$SUDO_INVOKER" ] && [ -x "/home/$SUDO_INVOKER/.local/bin/$1" ] && return 0
    [ -x "/usr/local/bin/$1" ] && return 0
    [ -x "/snap/bin/$1" ] && return 0
    return 1
}

PM=""
if cmd_exists apt; then
    PM=apt
elif cmd_exists pacman; then
    PM=pacman
elif cmd_exists dnf; then
    PM=dnf
elif cmd_exists yum; then
    PM=yum
fi

echo "Detected package manager: ${PM:-none}"

ensure_snapd() {
    if cmd_exists snap; then
        echo "snap already present"
        return 0
    fi
    case "$PM" in
        apt)
            apt update || true
            apt install -y snapd || return 1
            systemctl enable --now snapd.socket || true
            ;;
        pacman)
            pacman -Syu --noconfirm snapd || return 1
            systemctl enable --now snapd.socket || true
            ln -s /var/lib/snapd/snap /snap || true
            ;;
        dnf)
            dnf install -y snapd || return 1
            systemctl enable --now snapd.socket || true
            ;;
        yum)
            yum install -y snapd || return 1
            systemctl enable --now snapd.socket || true
            ;;
        *)
            echo "No supported package manager detected; please install snapd manually if needed" >&2
            return 1
            ;;
    esac
    # allow snapd a moment to initialize
    sleep 3
}

install_whisper_cpp() {
    if cmd_exists whisper-cli || cmd_exists whisper-stream || cmd_exists whisper-cpp; then
        echo "whisper-cli/whisper-stream/whisper-cpp already present; skipping whisper.cpp install"
        return 0
    fi

    if ! cmd_exists snap; then
        echo "snap not available; attempting to install snapd"
        ensure_snapd || { echo "snapd install failed; skipping whisper.cpp install" >&2; return 1; }
    fi

    echo "Installing whisper-cpp via snap..."
    snap install whisper-cpp || { echo "snap install whisper-cpp failed" >&2; return 1; }

    # Create aliases so users can call canonical names like `whisper-cli`.
    # Ignore errors if aliases already exist.
    snap alias whisper-cpp.cli whisper-cli || true
    snap alias whisper-cpp.download-ggml-model whisper-download-ggml-model || true
    snap alias whisper-cpp.download-vad-model whisper-download-vad-model || true

    # If there are any stale filesystem symlinks created previously at /usr/local/bin,
    # remove them only if they are broken or clearly point to an old non-snap path.
    for f in /usr/local/bin/whisper-cli /usr/local/bin/whisper-download-ggml-model /usr/local/bin/whisper-download-vad-model; do
        if [ -L "$f" ]; then
            target=$(readlink -f "$f" || true)
            # If target is not under /snap or is missing, remove the symlink to prefer snap alias.
            if [ -z "$target" ] || [[ "$target" != /snap/* && "$target" != /var/lib/snapd/* ]]; then
                echo "Removing stale symlink $f -> $target"
                rm -f "$f" || true
            fi
        fi
    done
    return 0
}

install_ffmpeg() {
    if cmd_exists ffmpeg; then
        echo "ffmpeg already present; skipping"
        return 0
    fi
    case "$PM" in
        apt)
            apt update || true
            apt install -y ffmpeg || { echo "apt ffmpeg install failed" >&2; return 1; }
            ;;
        pacman)
            pacman -S --noconfirm ffmpeg || { echo "pacman ffmpeg install failed" >&2; return 1; }
            ;;
        dnf)
            dnf install -y ffmpeg || echo "dnf ffmpeg may require additional repos" >&2
            ;;
        yum)
            yum install -y ffmpeg || echo "yum ffmpeg may require rpmfusion" >&2
            ;;
        *)
            echo "No package manager detected to install ffmpeg; please install it manually" >&2
            return 1
            ;;
    esac
}

install_yt_dlp() {
    if cmd_exists yt-dlp; then
        echo "yt-dlp already present; skipping"
        return 0
    fi

    # 1) Try OS package managers where package naming is known
    case "$PM" in
        apt)
            apt update || true
            if apt install -y yt-dlp 2>/dev/null; then
                echo "yt-dlp installed via apt"
                return 0
            fi
            ;;
        pacman)
            if pacman -Sy --noconfirm yt-dlp 2>/dev/null; then
                echo "yt-dlp installed via pacman"
                return 0
            fi
            ;;
    esac

    # 2) Try pipx (preferred for user-level Python apps)
    if ! cmd_exists pipx; then
        echo "pipx not found; attempting to install pipx via package manager"
        case "$PM" in
            apt)
                apt update || true
                apt install -y python3-pipx pipx || true
                ;;
            pacman)
                pacman -Sy --noconfirm python-pipx || true
                ;;
            dnf|yum)
                $PM install -y python3-pipx || true
                ;;
        esac
    fi

    if cmd_exists pipx; then
        pipx install yt-dlp || echo "pipx yt-dlp install warned or failed" >&2
        if cmd_exists yt-dlp; then
            echo "yt-dlp installed via pipx"
            return 0
        fi
    fi

    # 3) Fall back to pip --user. This may be blocked on some distributions
    # (PEP 668 / externally-managed). Detect that and print actionable guidance.
    if cmd_exists python3; then
        echo "Attempting 'python3 -m pip install --user yt-dlp'"
        set +e
        out=$(python3 -m pip install --user -U yt-dlp 2>&1)
        rc=$?
        set -e
        if [ $rc -eq 0 ]; then
            echo "yt-dlp installed via pip --user"
            # Ensure PATH earlier contains the local bin location
            return 0
        fi
        if echo "$out" | grep -qi "externally-managed-environment\|PEP 668"; then
            echo "pip install failed due to externally-managed environment (PEP 668)." >&2
            echo "Please install yt-dlp via your package manager (pacman/apt) or via pipx." >&2
            echo "Example (recommended): sudo pacman -S yt-dlp    OR    sudo apt install yt-dlp    OR    pipx install yt-dlp" >&2
            return 1
        fi
        echo "pip install output:\n$out" >&2
    fi

    echo "yt-dlp not installed automatically; please install yt-dlp manually" >&2
    return 1
}

download_default_model() {
    TARGET_DIR="${ZEROCLAW_WHISPER_MODELS_DIR:-$HOME/.zeroclaw/models}"
    mkdir -p "$TARGET_DIR"
    MODEL="ggml-medium.en-q5_0.bin"
    URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/$MODEL"
    if [ -f "$TARGET_DIR/$MODEL" ]; then
        echo "Model $MODEL already present in $TARGET_DIR"
        return 0
    fi
    echo "Downloading $MODEL into $TARGET_DIR (this may take a while)"
    if cmd_exists curl; then
        curl -L -o "$TARGET_DIR/$MODEL" "$URL" || { echo "curl download failed" >&2; return 1; }
    elif cmd_exists wget; then
        wget -O "$TARGET_DIR/$MODEL" "$URL" || { echo "wget download failed" >&2; return 1; }
    else
        echo "Neither curl nor wget available to fetch model" >&2
        return 1
    fi
    chmod a+r "$TARGET_DIR/$MODEL" || true
}

echo "Starting bootstrap: installing tools where missing (ffmpeg, yt-dlp, whisper-cpp)"

# Install ffmpeg
install_ffmpeg || echo "ffmpeg install step failed or skipped"

# Install yt-dlp
install_yt_dlp || echo "yt-dlp install step failed or skipped"

# Install whisper.cpp via snap (idempotent)
install_whisper_cpp || echo "whisper.cpp install failed or skipped"

# Download default model
download_default_model || echo "model download failed or skipped"

echo "Bootstrap complete."
