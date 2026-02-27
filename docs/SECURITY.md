# Security Model

## The Three Laws (why not?)

1. **The AI never touches the world directly.** Every write operation requires explicit human approval.
2. **The AI never sees raw credentials.** The Membrane layer handles all credential access.
3. **Untrusted content is never executable.** External content is quarantined within `<untrusted>` boundaries.

## Defense Layers

### Capability Enum (Compile-Time)
The `Capability` enum has no `Other(String)` variant. If the LLM returns something that doesn't deserialize into a known variant, it's dropped.

### Membrane (Runtime)
All credential access flows through the Membrane. The LLM never constructs HTTP requests or sees API keys.

### Policy Engine (Runtime)
Rate limiting, quiet hours, and per-capability approval rules enforce boundaries.

### Audit Log
Every action is logged with decision rationale, enabling forensic analysis.

### Filesystem Isolation (OS-Level)
systemd `InaccessiblePaths` provides kernel-level isolation between household members.

## Prompt Injection Defense

- External content wrapped in `<untrusted>` tags
- LLM responses parsed as typed structs (serde), not interpreted as commands
- Parse failures are logged and dropped: this IS the injection defense
- No shell execution capability exists in the enum
