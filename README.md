# Sentinel

**A secure personal AI assistant.**

Sentinel is a self-hosted personal AI assistant that runs as a background daemon on a Linux machine. It watches the email, calendar, tasks, weather, and messages and then uses an AI language model to generate notifications, morning briefings, and contextual alerts. Every action the AI wants to take requires the explicit user's approval. Nothing leaves then machine unless the user says so.

> **Works with several LLMs.** Sentinel is designed, prompt-engineered, and tested against Anthropic's Claude, but the AI layer is fully provider-agnostic. Any OpenAI-compatible provider works out of the box: OpenAI, DeepSeek, Gemini, Grok, Mistral, Together, Ollama, and more. Pointing it at any `/v1/chat/completions` endpoint makes it work.

> **No storage in the cloud and no data sharing.** The emails, calendar, habits, and memories stay on the local hardware, encrypted at rest. The only outbound call is to the AI provider's API, and even that only receives the context defined by the user.

> **Customizable.** Sentinel is the default name, but the assistant can be named differently by setting the `assistant_name` in the config and then every surface adapts: notifications, system prompt, onboarding and CLI output.

---

<!-- SCREENSHOT1 -->

---

## Table of Contents

- [Features](#features)
- [Architecture](#architecture)
- [Prerequisites](#prerequisites)
- [Installation](#installation)
- [Configuration](#configuration)
- [First Run & Onboarding](#first-run--onboarding)
- [Running as a Service](#running-as-a-service)
- [CLI Reference](#cli-reference)
- [Watchers](#watchers)
- [Household Mode](#household-mode)
- [Integrations](#integrations)
- [Feature Flags](#feature-flags)
- [Security Model](#security-model)
- [Privacy](#privacy)
- [Self-Hosting Guide](#self-hosting-guide)
- [Contributing](#contributing)
- [License](#license)

---

## Features

### Daily intelligence
- **Morning briefing**: weather, calendar events, due tasks, travel alerts, rhythm nudges, and a rolling context of recent activity, assembled by the LLM every morning
- **Departure alerts**: monitors calendar events with locations, queries a self-hosted OSRM routing server, and alerts the user to when to leave, with a buffer for rain (+5 min) or snow (+10 min)
- **Email triage**: IMAP IDLE connection; classifies urgency, respects sender allowlists and blocklists, surfaces only what matters
- **Task management**: flexible task scheduling with five schedule types: one-off (`Once`), recurring via RRULE string (`Recurring`), business-day-aware (`BusinessDay` with specs like `FirstOfMonth`, `LastFridayOfMonth`, `FirstOfQuarter`), condition-`Triggered`, and `RelativeToEvent` (offset from a calendar event); alerts on due and overdue items
- **Scheduled reflections**: daily, weekly, and monthly reflection prompts based on the user's own ledger of activity

### Habit and rhythm engine
- Tracks the user's activity patterns (meals cooked, tasks completed, events attended, etc.) via an append-only ledger
- Computes rhythm intervals using median absolute deviation (MAD) to be robust to outliers
- Classifies each activity as *On Track*, *Coming Up*, *Overdue*, *Dormant*, or *Emerging*
- Feeds rhythm data into every briefing so the LLM can notice when something is slipping

### Signal integration
- Receives messages from approved contacts and passes them to the LLM for context-aware replies
- Sends all notifications directly to the user's phone via Signal (primary channel)
- Approval requests for sensitive actions are sent as Signal messages: reply `yes <id>` or `no <id>`
- Full onboarding conversation conducted via Signal when the user first sets up Sentinel

### Sports and culture
- Loads season schedules from TOML files for motorsport, football, tennis, and other series
- Configurable per-series notification policy: `race_only`, `each_session`, or `weekly_mention`
- Optional spoiler protection flag for when the user is watching on delay
- Monitors RSS/Atom feeds and iCal calendars for local cultural events
- Scores events against the user's taste profile (artists, genres, venues) and feasibility (days until, distance)

### Weather
- Polls Open-Meteo (free, no API key) for current conditions and a 3-day forecast
- Weather conditions are shared with the departure watcher to apply travel time buffers
- WMO weather code descriptions in human-readable form

### Household mode
- Multiple household members each run their own isolated Sentinel instance
- Shared surface: family calendar, shopping list, meal plan, household task list
- Shopping list syncs with Bring! (live API); partner attribution tracked; the user is told when they remove something the partner added
- systemd `InaccessiblePaths` provides kernel-level filesystem isolation between instances

### Human-in-the-loop action model
- The LLM can only take actions within the `Capability` enum; there are 17 variants with no wildcards, no shell execution, and no filesystem writes
- Each capability is either auto-approved (by policy), sent for human approval via Signal, or blocked
- All decisions are written to an append-only JSON-lines audit log with reasoning and token cost
- Monthly AI spend tracked and displayed; configurable budget alerting

### Provider-agnostic AI
- Built-in support for Anthropic (native API), plus any OpenAI-compatible provider via a generic adapter
- Well-known defaults for 8 providers: Anthropic, OpenAI, DeepSeek, Gemini, Grok, Mistral, Together, Ollama
- Any custom provider works: just set `provider`, `model`, and `api_base` in the config
- Dynamic credential lookup: `SENTINEL_{NAME}_API_KEY` or `{NAME}_API_KEY` environment variables
- Ollama runs fully local with no API key required

### Configurable identity
- Name of the assistant to anything via `assistant_name` in the config
- Every user-facing surface adapts: desktop notifications, system prompt, onboarding conversation, CLI output
- Defaults to "Sentinel" when not set

### Web dashboard
- Optional local-only HTTP dashboard (`--dashboard-port`)
- Endpoints: `/api/health`, `/api/memories`, `/api/ledger`, `/api/cost`, `/api/engagement`
- CORS enabled for local access; bound to 127.0.0.1 only

---

## Architecture

```
┌───────────────────────────────────────────────────────────┐
│                       sentinel-cli                        │
│           (daemon event loop + CLI subcommands)           │
└────────────────────────────┬──────────────────────────────┘
                             │ WatchEvent
         ┌───────────────────▼───────────────────┐
         │           sentinel-watchers           │
         │  email · caldav · departure · signal  │
         │  weather · sports · cultural · tasks  │
         │  time (schedule runner)               │
         └───────────────────┬───────────────────┘
                             │
         ┌───────────────────▼───────────────────┐
         │           sentinel-cortex             │
         │  local triage → state compiler →      │
         │  prompt builder → AI provider →       │
         │  response parser → mode tracker       │
         └───────────────────┬───────────────────┘
                             │
         ┌───────────────────▼───────────────────┐
         │          sentinel-membrane            │
         │  policy engine · audit log ·          │
         │  credential vault · AES-256-GCM       │
         └───────────────────┬───────────────────┘
                             │
         ┌───────────────────▼───────────────────┐
         │            sentinel-gate              │
         │  notification router · Signal client  │
         │  desktop notifier · approval manager  │
         └───────────────────┬───────────────────┘
                             │
         ┌───────────────────▼───────────────────┐
         │           sentinel-memory             │
         │  SQLite/WAL · ledger · rhythms ·      │
         │  state · tasks · household · GDPR     │
         └───────────────────────────────────────┘
```

<!-- DIAGRAM -->

---

## Prerequisites

| Dependency | Notes |
|---|---|
| Linux x86_64 | Native platform. For macOS/Windows, use [Docker](#docker). |
| Rust (stable) | Build from source; `rustup` recommended. Not needed for Docker. |
| [signal-cli](https://github.com/AsamK/signal-cli) | Daemon mode with `--http` (sending) and `--socket` (receiving); default port 8083 |
| [Radicale](https://radicale.org/) | CalDAV server for calendar sync (or any CalDAV server) |
| [OSRM](http://project-osrm.org/) | Self-hosted routing (required for departure alerts) |
| AI provider API key | Anthropic (recommended), or any OpenAI-compatible provider. Ollama runs locally with no key. Any `/v1/chat/completions` endpoint works. |
| Java 17+ | Required by signal-cli |

**OSRM and Signal are optional**: Sentinel starts and runs without them, skipping the corresponding watchers.

---

## Installation

### Option A: Docker (easiest)

The fastest way to get running. Requires only Docker and Docker Compose.

```bash
git clone https://github.com/franciscomachado/Sentinel.git
cd sentinel/docker
cp .env.example .env        # fill in the API keys
vim sentinel.toml            # edit config
docker compose up -d
```

Dashboard at `http://127.0.0.1:8765`. See [docker/](docker/) for full instructions, OSRM map preparation, and signal-cli registration.

If the user already runs Radicale / OSRM / signal-cli on the host:

```bash
docker compose -f compose.standalone.yml up -d
```

### Option B: From source (bare-metal)

The fastest way is the interactive install script, which handles building,
signal-cli setup, config scaffolding, and systemd services:

```bash
git clone https://github.com/franciscomachado/Sentinel.git
cd sentinel
./install.sh
```

Or step by step:

#### 1. Install system dependencies

**Arch Linux:**
```bash
sudo pacman -S base-devel rustup java-runtime radicale signal-cli
rustup default stable
```

**Debian / Ubuntu:**
```bash
sudo apt install build-essential curl default-jre python3-pip
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
pipx install radicale
# signal-cli: download the latest release from github.com/AsamK/signal-cli
```

#### 2. Clone and build

```bash
git clone https://github.com/franciscomachado/Sentinel.git
cd sentinel
cargo build --release
```

The binary is at `target/release/sentinel`.

#### 3. Install the binary

```bash
sudo install -Dm755 target/release/sentinel /usr/local/bin/sentinel
```

Or install directly via Cargo:
```bash
cargo install --path crates/sentinel-cli
```

#### 4. Set up signal-cli

There are two ways to connect signal-cli to a Signal account:

**A) Link as secondary device** (most common if the user already uses Signal on their phone):
```bash
signal-cli link -n "Sentinel"
# Signal → Settings → Linked Devices → Link New Device
```

**B) Register as primary device** (dedicated number, e.g. a second SIM or VoIP):
```bash
signal-cli -a +PHONE_NUMBER register
signal-cli -a +PHONE_NUMBER verify CODE   # code received via SMS/call
```

Test that it works, then start the daemon with **both** `--http` (for sending)
and `--socket` (for receiving via subscriptions):

```bash
signal-cli -a +PHONE_NUMBER receive      # quick test - Ctrl-C after a few seconds
signal-cli -a +PHONE_NUMBER daemon --http localhost:8083 --socket
```

To run signal-cli as a systemd service, see [docs/SELF-HOSTING.md](docs/SELF-HOSTING.md).

#### 5. Set up Radicale (CalDAV)

Full instructions at [docs/RADICALE.md](docs/RADICALE.md). Quick start:

```bash
pipx install radicale
# Create ~/.config/radicale/config (see docs/RADICALE.md for template)
systemctl --user enable --now radicale
```

Sync phone to Radicale using [DAVx⁵](https://www.davx5.com/) (Android) or the built-in CalDAV account (iOS).

---

## Configuration

### Create the config directory

```bash
mkdir -p ~/.config/sentinel
cp config/sentinel.example.toml ~/.config/sentinel/sentinel.toml
cp config/policies.example.toml ~/.config/sentinel/policies.toml
```

Sentinel also checks the path in `$SENTINEL_CONFIG` if set.

### Minimal configuration

Edit `~/.config/sentinel/sentinel.toml`:

```toml
[user]
name = "Your Name"
timezone = "Europe/Madrid"    # or Europe/London, US/Eastern, etc.
locale = "pt-PT"
country = "PT"
# assistant_name = "Sentinel"   # customize your assistant's display name

# AI provider, defaults to Anthropic (Claude) if omitted.
# Sentinel is tuned for Claude; any OpenAI-compatible provider also works.
# [ai]
# provider = "anthropic"       # or "openai", "deepseek", "gemini", "groq", "ollama", etc.
# model = "claude-sonnet-4-20250514"
# api_base = "..."             # custom endpoint for unknown providers

[[email.accounts]]
name = "personal"
imap_host = "mail.example.com"
imap_port = 993

[calendar]
caldav_url = "http://127.0.0.1:5232/<your-radicale-username>/calendar.ics/"

[signal]
enabled = true
account = "+351XXXXXXXXX"           # the Signal number
port = 8083                            # signal-cli listening port (default: 8083)
allow_from = ["+351YYYYYYYYY"]      # User's phone number, the only one Sentinel listens to

[routing]
provider = "osrm"
endpoint = "http://localhost:5000"  # skip this block if not using departure alerts
```

### Credentials

All credentials are read from environment variables and nothing secret goes in the config file:

| Variable | Purpose |
|---|---|
| `ANTHROPIC_API_KEY` | Required if using Anthropic (default provider) |
| `SENTINEL_{NAME}_API_KEY` | Required for other providers (e.g. `DEEPSEEK_API_KEY`, `OPENAI_API_KEY`) |
| `SENTINEL_MODEL` | Optional, model name override (any provider) |
| `SENTINEL_DASHBOARD_BIND` | Dashboard bind address (default: `127.0.0.1`; Docker sets `0.0.0.0`) |
| `SENTINEL_CALDAV_USER` | CalDAV username |
| `SENTINEL_CALDAV_PASS` | CalDAV password |
| `SENTINEL_IMAP_PASS_<ACCOUNT>` | IMAP password per account (e.g. `SENTINEL_IMAP_PASS_PERSONAL`) |
| `SENTINEL_SIGNAL_URL` | Override signal-cli HTTP URL (default: built from `port`, e.g. `http://127.0.0.1:8083/api/v1/rpc`) |
| `BRING_EMAIL` / `BRING_PASSWORD` | Bring! shopping list credentials (optional) |

To be set in the shell profile or, for systemd, in a drop-in file (see [docs/SELF-HOSTING.md](docs/SELF-HOSTING.md)).

### Policy configuration

Copy `config/policies.example.toml` to `~/.config/sentinel/policies.toml` and tune:

```toml
[policy.quiet_hours]
start = "22:30"
end = "07:00"
except = ["urgent"]         # urgent notifications still come through

[policy.spending]
monthly_ai_budget_euros = 10.00
warn_at_percentage = 80

[policy.bring]
auto_approve_ai_suggested = false    # always ask before adding to shopping list
notify_partner_on_removal = true
```

---

## First Run & Onboarding

Verification of the configuration before starting:

```bash
sentinel check
```

This checks the config file, LLM key, and database path. Sample output:
```
Config path: /home/you/.config/sentinel/sentinel.toml
Configuration OK
  User: Your Name
  Assistant: Sentinel
  Timezone: Europe/Lisbon
  Locale: pt-PT
  AI provider: anthropic
  AI model: claude-sonnet-4-20250514 — OK
  Database: /home/you/.local/share/sentinel/sentinel.db
  Database status: will be created on first run
```

### Onboarding wizard

Run the interactive onboarding to build Sentinel's initial memory of the household, food preferences, schedule, and interests. The conversation happens entirely via Signal:

```bash
sentinel onboard
```

<!-- SCREENSHOT2 -->

This takes about 10 minutes. Everything shared is stored as memories in the local SQLite database and used to personalise every future briefing. The user can skip it and let Sentinel learn over time instead.

### Test the pipeline

Run a test event through the full pipeline (triage → LLM → policy → notification) without starting the daemon:

```bash
# Morning briefing
sentinel test-event morning-briefing

# Email triage
sentinel test-event email

# Departure alert
sentinel test-event departure

# Signal message
sentinel test-event signal

# Weather update
sentinel test-event weather
```

---

## Running as a Service

### systemd (recommended)

Copy the service file:

```bash
sudo cp systemd/sentinel@.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now sentinel@yourusername
```

The service runs as a dedicated `sentinel-yourusername` system user with a hardened profile:
- `ProtectSystem=strict`: filesystem is read-only except your data directory
- `PrivateTmp=true`: isolated `/tmp`
- `NoNewPrivileges=true`: privilege escalation is blocked
- `MemoryDenyWriteExecute=true`: no JIT or self-modifying code

Environment variables (credentials) go in a drop-in:

```bash
sudo mkdir -p /etc/systemd/system/sentinel@yourusername.service.d/
sudo tee /etc/systemd/system/sentinel@yourusername.service.d/env.conf <<EOF
[Service]
Environment="ANTHROPIC_API_KEY=sk-ant-..."
# Or for OpenAI: Environment="OPENAI_API_KEY=sk-..."
Environment="SENTINEL_CALDAV_PASS=yourpassword"
EOF
sudo systemctl restart sentinel@yourusername
```

### Run manually (foreground)

```bash
export ANTHROPIC_API_KEY=sk-ant-...
sentinel run
```

With the optional web dashboard:

```bash
sentinel run --dashboard-port 8765
# Dashboard available at http://127.0.0.1:8765
```

<!-- SCREENSHOT3 -->

### View logs

Bare-metal:
```bash
journalctl -u sentinel@yourusername -f
```

Docker:
```bash
docker compose -f docker/compose.yml logs -f sentinel
```

---

## CLI Reference

```
sentinel [--config <path>] <COMMAND>
```

| Command | Description |
|---|---|
| `run [--dashboard-port <port>]` | Start the daemon |
| `onboard` | Run the Signal-based onboarding wizard |
| `check` | Validate config and credentials |
| `test-event <kind>` | Push a test event through the AI pipeline |
| `travel set <dest> --from <date> --to <date>` | Activate travel mode |
| `travel clear` | Deactivate travel mode |
| `travel status` | Show current travel mode |
| `memory list` | List all stored memories |
| `memory search <query>` | Search memories |
| `memory delete <id>` | Delete a memory by ID |
| `memory delete-tag <tag>` | Delete all memories with a tag |
| `memory delete-before <days>` | Delete memories older than N days |
| `memory stats` | Count memories and observations |
| `ledger recent [n]` | Show the last N ledger entries |
| `ledger search <query>` | Search the activity ledger |
| `ledger stats` | Category breakdown |
| `ledger purge --older-than <days>` | Purge old entries |
| `cost [--month YYYY-MM]` | Show AI token usage and estimated cost |
| `export --format json` | GDPR export of all data |
| `reset --confirm` | Delete all data and start fresh |
| `household init` | Initialise the shared household database |
| `household add-member <name>` | Add a member to the household |
| `household setup shopping [--provider <name>]` | Configure shared shopping list provider |
| `household setup calendar --family-caldav-url <url>` | Configure shared family calendar |
| `household status` | Show shopping list, meal plan, member list |
| `validate-data <path> --type holiday` | Validate a holiday TOML file |
| `validate-data <path> --type sports` | Validate a sports season file |

---

## Watchers

Sentinel's input layer consists of independent async watchers that emit events into a shared channel. Each is only started if its section is present in the config.

| Watcher | Trigger | Notes |
|---|---|---|
| **Email** | IMAP IDLE (push) | Per-account TLS connections; urgency classification via glob patterns |
| **CalDAV** | PROPFIND ctag poll | Detects changes via etag diff; full iCal parser with RFC 5545 line unfolding |
| **Departure** | Interval check | Reads calendar events with locations; queries OSRM; applies weather buffer |
| **Signal** | JSON-RPC poll | Allowlist enforced; group policy configurable |
| **Weather** | Hourly poll | Open-Meteo; shares conditions with departure watcher |
| **Sports** | Computed from schedule | TOML season files in `data/sports/`; per-series notify policy |
| **Cultural events** | RSS/Atom + iCal poll | Taste profile scoring; feasibility window |
| **Tasks** | 60-second poll | Surfaces due and overdue tasks from local task store |
| **Time** | Schedule runner | Fires daily briefings, weekly planning, monthly reflections, rhythm engine |

### Sports data files

Season schedules live in `data/sports/` as TOML files. The format is validated by `sentinel validate-data`:

```toml
[series]
id = "f1"
name = "Formula 1"
timezone_offset_hours = 0

[[rounds]]
name = "Bahrain Grand Prix"

[[rounds.sessions]]
name = "Race"
start = "2025-03-02T15:00:00"
```

Contribute new series or update existing season files via pull request.

---

## Household Mode

Each household member runs their own isolated Sentinel instance. The instances share a single SQLite database for the collaborative surface:

```
├── /home/john/.local/share/sentinel/sentinel.db   (private)
├── /home/mary/.local/share/sentinel/sentinel.db         (private)
└── /srv/sentinel/household.db                           (shared)
```

Setup:

```bash
# One-time: initialise the shared DB
sentinel household init

# Add members
sentinel household add-member john
sentinel household add-member mary

# Connect Bring! shopping list
sentinel household setup shopping --provider bring

# Connect a shared family calendar
sentinel household setup calendar --family-caldav-url http://127.0.0.1:5232/family/calendar/
```

Partner attribution works across the shopping list: if Mary added "olive oil" and you ask Sentinel to remove it, you get a confirmation before it's removed, and Mary's Sentinel is notified.

---

## Integrations

### Default stack (all self-hosted or free)

| Integration | Protocol | Key required |
|---|---|---|
| IMAP email | IMAP IDLE over TLS | No |
| CalDAV calendar | CalDAV (PROPFIND/REPORT) | No |
| Signal messaging | JSON-RPC (signal-cli) | No |
| Weather | [Open-Meteo](https://open-meteo.com/) HTTP API | No |
| Routing | [OSRM](http://project-osrm.org/) HTTP API | No |
| Bring! shopping | Bring! REST API v2 | Yes (email/password) |

### Optional integration crates (placeholder)

The following crates implement the `Integration` trait and are ready for contributors:

| Crate | Status |
|---|---|
| `sentinel-gmail` | Stub ready for implementation |
| `sentinel-google-calendar` | Stub ready for implementation |
| `sentinel-todoist` | Stub ready for implementation |
| `sentinel-telegram` | Stub ready for implementation |

To implement an integration, fill in the `watch()` and `execute()` methods of the `Integration` trait. See [docs/INTEGRATIONS.md](docs/INTEGRATIONS.md) for the interface.

---

## Feature Flags

Sentinel uses Cargo feature flags as a proto-plugin system. Optional integrations can be compiled in or out without changing any configuration; if the feature is disabled, the code doesn't exist in the binary.

| Feature | Default | What it gates |
|---|---|---|
| `bring` | **on** | Bring! shopping list integration (household mode) |

Build without optional integrations:

```bash
cargo build --release --no-default-features
```

Build with everything (the Docker image does this):

```bash
cargo build --release --all-features
```

New integrations follow the same pattern: add the crate as an optional dependency, gate it behind a feature flag, and the daemon conditionally starts the watcher and wires up the executor. No runtime plugin loading and no dynamic libraries, just conditional compilation.

---

## Security Model

Sentinel is built around three rules:

1. **The AI never touches the world directly.** Every write operation requires explicit human approval or an auto-approval policy you define.
2. **The AI never sees raw credentials.** The Membrane layer handles all credential access. The LLM never constructs HTTP requests or sees API keys.
3. **Untrusted content is never executable.** External content (emails, Signal messages) is wrapped in `<untrusted>` tags in the prompt. LLM responses are parsed as typed Rust structs and parse failures are silently dropped.

### Capability firewall

The `Capability` enum is the complete list of things the AI can ask to do:

```
// Reads (auto-approved)
EmailRead · CalendarRead · TaskListRead · WeatherFetch · RoutingQuery

// Auto-approved writes (reversible / low-risk)
TaskCreate · TaskComplete · TaskModify
DishAdd · MealPlanSet
CalendarEventCreate · CalendarEventModify
ReminderSet

// Require human approval
CalendarEventDelete
BringAdd · BringRemove
EmailDraft · EmailReply · SignalReply
```

There is no `Other(String)` variant, no `ExecuteCommand` and no `FileWrite`. There is no `EmailSend`, only `EmailDraft`, which requires human approval before anything is actually sent. If the model returns something outside this enum, it is dropped at parse time.
### Approval flow

<!-- SCREENSHOT4 -->

When the AI requests an action that requires approval, Sentinel sends a Signal message with an action ID. Reply `yes <id>` to approve or `no <id>` to reject. Approvals expire automatically.

### Audit log

Every action, either approved, blocked, or auto-approved, is written to a JSON-lines file with:
- The capability and its parameters
- The decision (auto-approved / human-approved / human-rejected / policy-blocked)
- LLM's reasoning- Token cost and estimated money spend

```bash
cat ~/.local/share/sentinel/audit.jsonl | jq .
```

---

## Privacy

- All data stays on your machine (memories, ledger, observations, task store, household database)
- The AI provider API receives only the state context you configure with no raw email bodies by default, only sender + subject + preview
- All persistent state is encrypted at rest with AES-256-GCM (random nonce per write)
- GDPR data export at any time: `sentinel export --format json`
- Configurable retention: ledger, audit log, and email cache each have independent `*_retention_days` settings
- Full wipe: `sentinel reset --confirm`

---

## Self-Hosting Guide

The full self-hosting guide covers signal-cli registration, OSRM setup, Radicale CalDAV, firewall configuration, and multi-user deployment: [docs/SELF-HOSTING.md](docs/SELF-HOSTING.md).

For Radicale specifically: [docs/RADICALE.md](docs/RADICALE.md).

---

## Contributing

Contributions are welcome. The most valuable areas right now:

- **Integration crates**: implementing `sentinel-gmail`, `sentinel-google-calendar`, `sentinel-todoist`, or `sentinel-telegram`
- **Bug reports**: especially edge cases in the iCal parser, CalDAV watcher, or IMAP reconnection logic

See [docs/CONTRIBUTING.md](docs/CONTRIBUTING.md) for the code style guide and PR process.

---

## License

MIT. See [LICENSE](LICENSE).

---

<sub>Sentinel is an independent open-source project. It is not affiliated with Anthropic, Signal, Bring!, or any other service mentioned here.</sub>
