# Contributing

## Code

Sentinel is written in Rust. The workspace has multiple crates.

## Holiday Data

Adding a country's holiday calendar is a TOML file contribution:

```
config/holidays/xx.toml
```

See existing files (pt.toml, de.toml, us.toml) for the format.

## Sports Data

Season calendars live in:

```
data/sports/<category>/<series>-<year>.toml
```

## Community Data

Both holiday and sports data files can be contributed via pull request and no code changes needed.
