# Radicale CalDAV Setup

Sentinel connects to any CalDAV server. [Radicale](https://radicale.org/) is the recommended self-hosted option; it's a single Python package with file-based storage, no database required.

## Install Radicale

```bash
# System-wide (Debian/Ubuntu)
sudo apt install python3-pip
pip3 install --user radicale

# Or via pipx (recommended)
pipx install radicale
```

## Configure

Create `~/.config/radicale/config`:

```ini
[server]
hosts = 127.0.0.1:5232

[auth]
type = htpasswd
htpasswd_filename = ~/.config/radicale/users
htpasswd_encryption = bcrypt

[storage]
filesystem_folder = ~/.local/share/radicale/collections

[logging]
level = info
```

Create the htpasswd file:

```bash
pip3 install --user bcrypt passlib  # if not already installed
htpasswd -B -c ~/.config/radicale/users john
```

## Run as a systemd service

Create `~/.config/systemd/user/radicale.service`:

```ini
[Unit]
Description=Radicale CalDAV Server
After=network.target

[Service]
ExecStart=%h/.local/bin/radicale
Restart=on-failure

[Install]
WantedBy=default.target
```

```bash
systemctl --user daemon-reload
systemctl --user enable --now radicale
```

Verify it's running:

```bash
curl -u john http://127.0.0.1:5232/.web/
```

## Create a calendar collection

Open `http://127.0.0.1:5232/.web/` in a browser, log in, and create a new calendar. Or use `curl`:

```bash
curl -u john -X MKCALENDAR http://127.0.0.1:5232/john/calendar.ics/
```

Your CalDAV URL for Sentinel will be:

```
http://127.0.0.1:5232/john/calendar.ics/
```

## Sentinel configuration

In `sentinel.toml`:

```toml
[calendar]
caldav_url = "http://127.0.0.1:5232/john/calendar.ics/"
username = "john"
# password via env: SENTINEL_CALDAV_PASS
poll_interval_secs = 120
```

Set the password:

```bash
export SENTINEL_CALDAV_PASS="your-radicale-password"
```

## Phone sync

### Android (DAVx⁵)

1. Install [DAVx⁵](https://www.davx5.com/) from F-Droid or Play Store
2. Add account → "Login with URL and username"
3. Base URL: `http://<your-server-ip>:5232/`  
   (use your LAN IP, not 127.0.0.1)
4. Enter your Radicale username and password
5. Select the calendar to sync

### iOS

1. Settings → Calendar → Accounts → Add Account → Other
2. Add CalDAV Account
3. Server: `<your-server-ip>:5232`
4. Username and password as configured in Radicale

## Network access

If syncing from a phone, Radicale must listen on your LAN interface:

```ini
[server]
hosts = 0.0.0.0:5232
```

For security, restrict access to your local network via firewall rules, or put Radicale behind a reverse proxy with TLS.
