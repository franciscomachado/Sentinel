# Integrations

## Default Stack (Self-Hosted)

| Need | Default | Protocol |
|---|---|---|
| Email | IMAP/SMTP | Standard |
| Calendar | Radicale | CalDAV |
| Tasks | Local engine | RRULE |
| Shopping | Bring | API |
| Messaging | signal-cli | Signal |
| Routing | OSRM | HTTP |
| Weather | Open-Meteo | HTTP |

## Optional Integrations

Available as feature-flagged crate:

- **sentinel-bring**: Bring! shopping lists

In the future, possibly:
- **sentinel-gmail**: Gmail via Google API + OAuth
- **sentinel-google-calendar**: Google Calendar API
- **sentinel-todoist**: Todoist API
- **sentinel-telegram**: Telegram Bot API

## Adding an Integration

Implement the `Integration` trait:

```rust
#[async_trait]
pub trait Integration: Send + Sync {
    fn id(&self) -> &str;
    fn category(&self) -> IntegrationCategory;
    fn capabilities(&self) -> Vec<CapabilityKind>;
    fn credential_requirements(&self) -> Vec<CredentialRequirement>;
    async fn validate(&self, creds: &Credentials) -> Result<()>;
    async fn watch(&self, ctx: WatcherContext) -> Result<()>;
    async fn execute(&self, action: &Capability) -> Result<ActionResult>;
}
```
