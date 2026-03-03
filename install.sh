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

# ─── PATH setup ──────────────────────────────────────────────────────

step "Shell PATH"

# Make sentinel reachable for the rest of this script immediately
export PATH="$HOME/.cargo/bin:$PATH"

# Persist into whichever RC files exist, skipping if already present
for _rc in "$HOME/.bashrc" "$HOME/.zshrc"; do
    if [[ -f "$_rc" ]] && ! grep -qF '.cargo/bin' "$_rc"; then
        printf '\n# Added by Sentinel installer\nexport PATH="$HOME/.cargo/bin:$PATH"\n' >> "$_rc"
        info "Added ~/.cargo/bin to PATH in $(basename "$_rc")"
    fi
done
warn "Open a new terminal (or run: source ~/.bashrc / source ~/.zshrc) for PATH to take effect outside this session"

# ─── Config ──────────────────────────────────────────────────────────

step "Setting up configuration"

mkdir -p "$CONFIG_DIR"

if [[ -f "$CONFIG_DIR/sentinel.toml" ]]; then
    info "Config already exists at $CONFIG_DIR/sentinel.toml — skipping setup wizard"
else

    # ── User profile ──────────────────────────────────────────────────
    step "User profile"
    _default_tz=$(timedatectl show --property=Timezone --value 2>/dev/null \
                  || cat /etc/timezone 2>/dev/null \
                  || echo "UTC")
    read -rp "  Full name: " _user_name
    read -rp "  Timezone [$_default_tz]: " _user_tz
    _user_tz="${_user_tz:-$_default_tz}"
    read -rp "  Locale (e.g. pt-PT, en-US) [en-US]: " _user_locale
    _user_locale="${_user_locale:-en-US}"
    read -rp "  Country code (e.g. PT, US, DE) [US]: " _user_country
    _user_country="${_user_country:-US}"
    read -rp "  Home city/municipality (for holidays, e.g. porto, berlin): " _user_home_region
    echo ""
    echo "  Home coordinates are used for weather and departure alerts."
    echo "  Leave blank to skip (configurable later in sentinel.toml)."
    read -rp "  Home latitude  (e.g. 41.1579): " _home_lat
    read -rp "  Home longitude (e.g. -8.6291): " _home_lon

    # ── AI provider ───────────────────────────────────────────────────
    step "AI provider"
    echo "  Supported: anthropic (default), openai, deepseek, gemini, groq, mistral, ollama"
    read -rp "  Provider [anthropic]: " _ai_provider
    _ai_provider="${_ai_provider:-anthropic}"
    read -rp "  Model override (leave blank for provider default): " _ai_model
    _ai_api_key=""
    if [[ "$_ai_provider" != "ollama" ]]; then
        read -rsp "  API key: " _ai_api_key
        echo ""
    fi

    # ── Email accounts ────────────────────────────────────────────────
    step "Email accounts"
    read -rp "  How many email accounts to configure? [1]: " _email_count
    _email_count="${_email_count:-1}"

    _email_accounts_toml=""
    _email_env_lines=""

    for (( _i=1; _i<=_email_count; _i++ )); do
        echo ""
        echo "  ── Email account $_i ──"
        read -rp "  Account name [personal]: " _em_name
        _em_name="${_em_name:-personal}"
        read -rp "  IMAP host: " _em_imap_host
        read -rp "  IMAP port [993]: " _em_imap_port
        _em_imap_port="${_em_imap_port:-993}"
        read -rp "  SMTP host [$_em_imap_host]: " _em_smtp_host
        _em_smtp_host="${_em_smtp_host:-$_em_imap_host}"
        read -rp "  SMTP port [587]: " _em_smtp_port
        _em_smtp_port="${_em_smtp_port:-587}"
        read -rp "  Username / email address: " _em_user
        read -rsp "  Password: " _em_pass
        echo ""
        _em_name_upper="${_em_name^^}"
        _email_accounts_toml+=$'\n[[email.accounts]]\n'"name = \"$_em_name\""$'\nimap_host = '"\"$_em_imap_host\""$'\nimap_port = '"$_em_imap_port"$'\nsmtp_host = '"\"$_em_smtp_host\""$'\nsmtp_port = '"$_em_smtp_port"
        _email_env_lines+="SENTINEL_EMAIL_${_em_name_upper}_USER=$_em_user"$'\n'
        _email_env_lines+="SENTINEL_EMAIL_${_em_name_upper}_PASS=$_em_pass"$'\n'
    done

    # ── CalDAV ────────────────────────────────────────────────────────
    step "Calendar (CalDAV)"
    _caldav_user=""
    _caldav_pass=""
    read -rp "  CalDAV username (leave blank if no auth): " _caldav_user
    if [[ -n "$_caldav_user" ]]; then
        read -rsp "  CalDAV password: " _caldav_pass
        echo ""
        _caldav_default_url="http://localhost:5232/${_caldav_user}/calendar.ics/"
    else
        _caldav_default_url="http://localhost:5232/user/calendar.ics/"
    fi
    read -rp "  CalDAV URL [$_caldav_default_url]: " _caldav_url
    _caldav_url="${_caldav_url:-$_caldav_default_url}"

    # ── Write sentinel.toml ───────────────────────────────────────────
    {
        printf '# Sentinel Configuration\n'
        printf '# Generated by install.sh on %s\n\n' "$(date +%F)"
        printf '[user]\n'
        printf 'name = "%s"\n' "$_user_name"
        printf 'timezone = "%s"\n' "$_user_tz"
        printf 'locale = "%s"\n' "$_user_locale"
        printf 'country = "%s"\n' "$_user_country"
        [[ -n "$_user_home_region" ]] && printf 'home_region = "%s"\n' "$_user_home_region"
        printf '\n[ai]\n'
        printf 'provider = "%s"\n' "$_ai_provider"
        [[ -n "$_ai_model" ]] && printf 'model = "%s"\n' "$_ai_model"
        [[ -n "$_email_accounts_toml" ]] && printf '%s\n' "$_email_accounts_toml"
        printf '\n[email.triage]\n'
        printf 'priority_senders = []\n'
        printf 'ignore_senders = ["*@newsletter.*", "noreply@*"]\n'
        printf 'preview_max_chars = 500\n'
        printf '\n[calendar]\n'
        printf 'caldav_url = "%s"\n' "$_caldav_url"
        if [[ -n "$_home_lat" && -n "$_home_lon" ]]; then
            printf '\n[weather]\n'
            printf 'lat = %s\n' "$_home_lat"
            printf 'lon = %s\n' "$_home_lon"
            printf '\n[departure]\n'
            printf 'home_lat = %s\n' "$_home_lat"
            printf 'home_lon = %s\n' "$_home_lon"
            printf '\n[routing]\n'
            printf 'provider = "osrm"\n'
            printf 'endpoint = "http://localhost:5000"\n'
        fi
        printf '\n[privacy]\n'
        printf 'ledger_retention_days = 365\n'
        printf 'audit_retention_days = 180\n'
        printf 'email_cache_retention_days = 30\n'
        printf 'memory_review_monthly = true\n'
        printf '\n[policy]\n'
        printf 'auto_approve_reads = true\n'
        printf 'max_writes_per_hour = 20\n'
    } > "$CONFIG_DIR/sentinel.toml"
    info "Created $CONFIG_DIR/sentinel.toml"

    # ── Write .env (owner-read only) ──────────────────────────────────
    # umask 077 inside the subshell ensures the file is created as mode 600
    # (no window where it is world-readable), and secrets are never echoed.
    _ai_var="${_ai_provider^^}_API_KEY"
    (
        umask 077
        {
            printf '# Sentinel secrets — keep this file private (mode 600)\n\n'
            [[ -n "$_ai_api_key" ]] && printf '%s=%s\n' "$_ai_var" "$_ai_api_key"
            if [[ -n "$_email_env_lines" ]]; then
                printf '\n# Email credentials\n'
                printf '%s' "$_email_env_lines"
            fi
            if [[ -n "$_caldav_user" ]]; then
                printf '\n# CalDAV credentials\n'
                printf 'SENTINEL_CALDAV_USER=%s\n' "$_caldav_user"
                printf 'SENTINEL_CALDAV_PASS=%s\n' "$_caldav_pass"
            fi
        } > "$CONFIG_DIR/.env"
    )
    info "Created $CONFIG_DIR/.env (mode 600)"

fi

# Source .env so credentials are available for the rest of this session
# (e.g. sentinel onboard, sentinel check) without requiring a shell restart
if [[ -f "$CONFIG_DIR/.env" ]]; then
    set -a
    # shellcheck source=/dev/null
    source "$CONFIG_DIR/.env"
    set +a
    info "Sourced $CONFIG_DIR/.env into current session"
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

# Try to detect the registered account (timeout guards against daemon lock hangs)
signal_account=""
_accounts_json=$(timeout 5 signal-cli -o json listAccounts 2>/dev/null || true)
if [[ -n "$_accounts_json" ]]; then
    signal_account=$(printf '%s' "$_accounts_json" | grep -oP '"\+[0-9]+"' | head -1 | tr -d '"' || true)
    [[ -n "$signal_account" ]] && info "Detected signal-cli account: $signal_account"
fi

if [[ -z "$signal_account" ]]; then
    read -rp "signal-cli account number (e.g. +351949594959): " signal_account
fi

# Append [signal] block now that the account number is confirmed
if [[ -f "$CONFIG_DIR/sentinel.toml" ]] && ! grep -q '^\[signal\]' "$CONFIG_DIR/sentinel.toml"; then
    {
        printf '\n[signal]\n'
        printf 'enabled = true\n'
        printf 'account = "%s"\n' "$signal_account"
        printf 'port = %s\n' "${signal_port:-42989}"
    } >> "$CONFIG_DIR/sentinel.toml"
    info "Added [signal] config to sentinel.toml"
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
# Pin EnvironmentFile to the actual config path (handles non-default XDG_CONFIG_HOME)
sed -i "s|EnvironmentFile=.*|EnvironmentFile=-${CONFIG_DIR}/.env|" "$SYSTEMD_DIR/sentinel.service"
info "Installed sentinel.service"

# Reload and enable
systemctl --user daemon-reload
systemctl --user enable signal-cli sentinel
info "Services enabled (signal-cli + sentinel)"

# ─── Start? ──────────────────────────────────────────────────────────

step "Ready"

echo ""
echo "  Before starting, make sure you've:"
echo "  1. Reviewed $CONFIG_DIR/sentinel.toml"
echo "  2. Verified secrets in $CONFIG_DIR/.env"
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
