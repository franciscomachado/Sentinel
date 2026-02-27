# Household Architecture

## Separate Instances, Shared Surface

Each household member gets their own isolated Sentinel instance with separate:
- Email access
- Memories and observations
- Rhythms and ledger
- Signal connection
- SQLite database

## Shared Surface

Only explicitly-shared data lives in the household database:
- Family calendar
- Shopping list (Bring)
- Meal plan
- Kids' schedule
- Household tasks/chores

## Setup

```bash
sentinel household init
sentinel household add-member john
sentinel household add-member mary

# Each member sets up independently
sentinel --user john setup ...
sentinel --user mary setup ...

# Shared integrations
sentinel household setup shopping --provider bring
sentinel household setup calendar --family-caldav-url ...
```

## Filesystem Isolation

systemd `InaccessiblePaths` provides kernel-level isolation. Each user's process literally cannot see the other user's data directory.
