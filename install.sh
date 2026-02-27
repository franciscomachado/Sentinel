#!/usr/bin/env bash
# install.sh — Sentinel deployment script
# Run on the target machine where Sentinel will run.
#
# Usage:
#   curl -sSfL https://raw.githubusercontent.com/.../install.sh | bash
#   # or locally:
#   ./install.sh
#
# Prerequisites: git, Java 17+, Rust toolchain (rustup)

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BOLD='\033[1m'
NC='\033[0m'

info()  { echo -e "${GREEN}[+]${NC} $*"; }
warn()  { echo -e "${YELLOW}[!]${NC} $*"; }
error() { echo -e "${RED}[✗]${NC} $*"; }
step()  { echo -e "\n${BOLD}── $* ──${NC}"; }

SENTINEL_DIR="${SENTINEL_DIR:-$HOME/sentinel}"
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/sentinel"
SYSTEMD_DIR="$HOME/.config/systemd/user"

# ─── Preflight checks ────────────────────────────────────────────────

step "Checking prerequisites"

missing=()
command -v git      >/dev/null || missing+=(git)
command -v cargo    >/dev/null || missing+=(rustup/cargo)
command -v java     >/dev/null || missing+=(java)
command -v signal-cli >/dev/null || missing+=(signal-cli)

if [[ ${#missing[@]} -gt 0 ]]; then
    error "Missing: ${missing[*]}"
    echo "Install them first. On Arch: sudo pacman -S base-devel rustup java-runtime signal-cli"
    echo "On Debian: sudo apt install build-essential default-jre; curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    echo "signal-cli: https://github.com/AsamK/signal-cli/releases"
    exit 1
fi

info "All prerequisites found"

# ─── Clone / update source ───────────────────────────────────────────

step "Getting Sentinel source"

if [[ -d "$SENTINEL_DIR/.git" ]]; then
    info "Updating existing checkout at $SENTINEL_DIR"
    git -C "$SENTINEL_DIR" pull --ff-only
else
    info "Cloning into $SENTINEL_DIR"
    git clone https://github.com/franciscomachado/Sentinel.git "$SENTINEL_DIR"
fi

# ─── Build ────────────────────────────────────────────────────────────

step "Building Sentinel (release)"

cd "$SENTINEL_DIR"
cargo build --release

info "Binary: $SENTINEL_DIR/target/release/sentinel"

# ─── Install binary ──────────────────────────────────────────────────

step "Installing binary"

cargo install --path crates/sentinel-cli
info "Installed to $(which sentinel 2>/dev/null || echo '$HOME/.cargo/bin/sentinel')"

# ─── Config ──────────────────────────────────────────────────────────

step "Setting up configuration"

mkdir -p "$CONFIG_DIR"

if [[ -f "$CONFIG_DIR/sentinel.toml" ]]; then
    info "Config already exists at $CONFIG_DIR/sentinel.toml — skipping"
else
    cp "$SENTINEL_DIR/config/sentinel.example.toml" "$CONFIG_DIR/sentinel.toml"
    info "Created $CONFIG_DIR/sentinel.toml from example"
    warn "Edit it now: \$EDITOR $CONFIG_DIR/sentinel.toml"
fi

if [[ ! -f "$CONFIG_DIR/.env" ]]; then
    cat > "$CONFIG_DIR/.env" <<'ENVEOF'
# API key for your AI provider (default: Anthropic)
ANTHROPIC_API_KEY=sk-ant-...

# Email credentials (per account)
# SENTINEL_EMAIL_PERSONAL_USER=you@example.com
# SENTINEL_EMAIL_PERSONAL_PASS=app-password

# CalDAV credentials (if Radicale uses auth)
# SENTINEL_CALDAV_USER=user
# SENTINEL_CALDAV_PASS=secret
ENVEOF
    info "Created $CONFIG_DIR/.env — add your API keys"
fi

# ─── signal-cli setup ────────────────────────────────────────────────

step "signal-cli setup"

echo ""
echo "  signal-cli needs to be connected to your Signal account."
echo "  There are two modes depending on your setup:"
echo ""
echo -e "  ${BOLD}A) Link as secondary device${NC} (you have Signal on your phone)"
echo "     This is the most common setup. Your phone remains the primary"
echo "     device and Sentinel acts as a linked device (like Signal Desktop)."
echo ""
echo "     signal-cli link -n \"Sentinel\""
echo "     → Displays a QR code or URI — scan it with Signal on your phone"
echo "       (Settings → Linked Devices → Link New Device)"
echo ""
echo -e "  ${BOLD}B) Register as primary device${NC} (dedicated number, no phone)"
echo "     Use this when you have a second phone number (SIM/VoIP) that"
echo "     is NOT registered on Signal yet. The number becomes Sentinel's own."
echo ""
echo "     signal-cli -a +YOURNUMBER register"
echo "     signal-cli -a +YOURNUMBER verify CODE"
echo "     → You'll receive an SMS/call with a verification code"
echo ""

read -rp "Has signal-cli already been set up for this machine? [y/N] " signal_ready

if [[ "${signal_ready,,}" != "y" ]]; then
    echo ""
    read -rp "Which mode? [A=link / B=register] " signal_mode
    echo ""

    case "${signal_mode,,}" in
        a|link)
            info "Running: signal-cli link -n \"Sentinel\""
            echo "Scan the QR code / URI with your phone (Settings → Linked Devices)."
            echo ""
            signal-cli link -n "Sentinel"
            ;;
        b|register)
            read -rp "Phone number (e.g. +351949594959): " signal_number
            info "Running: signal-cli -a $signal_number register"
            signal-cli -a "$signal_number" register || true
            echo ""
            read -rp "Verification code from SMS/call: " signal_code
            signal-cli -a "$signal_number" verify "$signal_code"
            ;;
        *)
            warn "Skipping — run signal-cli link or register manually"
            ;;
    esac
fi

# ─── Determine signal-cli account ────────────────────────────────────

# Try to detect the registered account
signal_account=""
if signal-cli -o json listAccounts 2>/dev/null | head -1 | grep -qoP '"number"\s*:\s*"\+[0-9]+"'; then
    signal_account=$(signal-cli -o json listAccounts 2>/dev/null | grep -oP '"\+[0-9]+"' | head -1 | tr -d '"')
    info "Detected signal-cli account: $signal_account"
fi

if [[ -z "$signal_account" ]]; then
    read -rp "signal-cli account number (e.g. +351949594959): " signal_account
fi

# ─── systemd services ───────────────────────────────────────────────

step "Installing systemd user services"

mkdir -p "$SYSTEMD_DIR"

# signal-cli service
if [[ -f "$SYSTEMD_DIR/signal-cli.service" ]]; then
    info "signal-cli.service already exists — checking for --socket flag"
    if ! grep -q -- '--socket' "$SYSTEMD_DIR/signal-cli.service"; then
        warn "Adding --socket to signal-cli.service (needed for subscriptions)"
        sed -i 's|daemon --http|daemon --http --socket|' "$SYSTEMD_DIR/signal-cli.service" 2>/dev/null || true
        # If the sed didn't match (different format), warn the user
        if ! grep -q -- '--socket' "$SYSTEMD_DIR/signal-cli.service"; then
            warn "Could not auto-patch — please add --socket to ExecStart manually"
        fi
    fi
else
    read -rp "signal-cli HTTP port [42989]: " signal_port
    signal_port="${signal_port:-42989}"

    cat > "$SYSTEMD_DIR/signal-cli.service" <<SIGEOF
[Unit]
Description=Signal CLI daemon
Wants=network-online.target
After=network-online.target

[Service]
Type=simple
ExecStartPre=/usr/bin/sleep 5
Environment="SIGNAL_CLI_OPTS=-Xms2m"
ExecStart=$(command -v signal-cli) -a $signal_account daemon --http 127.0.0.1:$signal_port --socket
Restart=always
RestartSec=10

[Install]
WantedBy=default.target
SIGEOF
    info "Created signal-cli.service (port $signal_port)"
fi

# Sentinel service
cp "$SENTINEL_DIR/systemd/sentinel.service" "$SYSTEMD_DIR/sentinel.service"
info "Installed sentinel.service"

# Reload and enable
systemctl --user daemon-reload
systemctl --user enable signal-cli sentinel
info "Services enabled (signal-cli + sentinel)"

# ─── Start? ──────────────────────────────────────────────────────────

step "Ready"

echo ""
echo "  Before starting, make sure you've:"
echo "  1. Edited $CONFIG_DIR/sentinel.toml"
echo "  2. Added API keys to $CONFIG_DIR/.env"
echo ""
read -rp "Start services now? [y/N] " start_now

if [[ "${start_now,,}" == "y" ]]; then
    systemctl --user start signal-cli
    sleep 3
    systemctl --user start sentinel
    info "Services started"
    echo ""
    systemctl --user status signal-cli sentinel --no-pager
else
    echo ""
    echo "  Start manually when ready:"
    echo "    systemctl --user start signal-cli"
    echo "    systemctl --user start sentinel"
fi

echo ""
info "Run onboarding: sentinel onboard"
info "Check status:   systemctl --user status sentinel"
info "View logs:      journalctl --user -u sentinel -f"
