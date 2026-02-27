# Self-Hosting Guide

## Hardware Requirements

- **CPU:** Negligible (sleeps 99.9% of the time)
- **RAM:** ~300MB total (Sentinel + Radicale + signal-cli)
- **Disk:** ~100-200MB
- **Network:** Persistent IMAP IDLE + occasional API calls

A Raspberry Pi 4, old laptop, or any always-on Linux box will work.

## Services

### Radicale (CalDAV)
Self-hosted calendar server. Android/iOS sync natively via CalDAV.

### signal-cli
Command-line Signal client for messaging integration.

### OSRM (Optional)
Self-hosted routing engine. Adds ~500MB RAM for a country extract. Use TomTom free tier if RAM is tight.

## External Dependencies

Only **one**: the Anthropic API for intelligence. Everything else is self-hosted.
