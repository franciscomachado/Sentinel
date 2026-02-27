# Setup Guide

## Prerequisites

- Linux system (always-on recommended)
- Rust toolchain
- Radicale (CalDAV server)
- signal-cli (Signal messaging)
- Anthropic API key

## Installation

```bash
# Install adjacent services
sudo pacman -S radicale
yay -S signal-cli

# Link Signal
signal-cli link -n "Sentinel"

# Install Sentinel
cargo install sentinel-cli

# Run onboarding (interactive conversation via Signal)
sentinel onboard

# Or manual setup
sentinel setup email personal --imap-host mail.example.com
sentinel setup calendar --caldav-url http://localhost:5232/user/default/
sentinel setup anthropic
sentinel setup routing --provider osrm --endpoint http://localhost:5000

# Start
sudo systemctl enable --now sentinel@yourusername
```

## Configuration

Copy example configs:
```bash
mkdir -p ~/.config/sentinel
cp config/sentinel.example.toml ~/.config/sentinel/sentinel.toml
cp config/policies.example.toml ~/.config/sentinel/policies.toml
```
