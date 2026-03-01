mod commands;
mod daemon;
mod setup;

use anyhow::Result;
use clap::Parser;
use commands::{Cli, Command};
use sentinel_core::config::SentinelConfig;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Command::Run { dashboard_port } => {
            let config = load_config(cli.config.as_deref())?;
            let pool = open_db().await?;

            // Optionally start the web dashboard
            if let Some(port) = dashboard_port {
                let dash_pool = pool.clone();
                tokio::spawn(async move {
                    let dashboard = sentinel_web::server::DashboardState {
                        ledger: sentinel_memory::ledger::Ledger::new(dash_pool.clone()),
                        state_mgr: sentinel_memory::state::StateManager::new(dash_pool.clone()),
                        audit: sentinel_membrane::audit::AuditLog::new(dash_pool),
                    };
                    if let Err(e) = sentinel_web::server::serve(dashboard, port).await {
                        tracing::error!(error = %e, "web dashboard failed");
                    }
                });
            }

            let d = daemon::Daemon::new(config, pool).await?;
            d.run().await
        }
        Command::Onboard => {
            let config = load_config(cli.config.as_deref())?;
            let pool = open_db().await?;

            let signal_config = config.signal.as_ref()
                .ok_or_else(|| anyhow::anyhow!("Signal must be configured for onboarding"))?;
            if !signal_config.enabled {
                anyhow::bail!("Signal is configured but not enabled");
            }
            let signal = sentinel_gate::signal::SignalClient::new(signal_config.clone());

            // Create provider from config (defaults to Anthropic)
            let vault = sentinel_membrane::credentials::CredentialVault::new();
            let provider = sentinel_cortex::provider::create_provider(
                config.ai.as_ref(),
                &vault,
            )?;

            println!("AI provider: {} (model: {})", provider.provider_name(), provider.model());
            println!("Note: {} is tuned for Claude. Results may vary with other providers.\n",
                config.user.assistant_name());

            let wizard = setup::SetupWizard::new(&config, signal, provider, pool)?;
            wizard.run().await
        }
        Command::Check => {
            let config_path = SentinelConfig::resolve_path(cli.config.as_deref());
            println!("Config path: {}", config_path.display());

            match SentinelConfig::load(&config_path) {
                Ok(config) => {
                    println!("Configuration OK");
                    println!("  User: {}", config.user.name);
                    println!("  Timezone: {}", config.user.timezone);
                    println!("  Locale: {}", config.user.locale);

                    // Check AI provider
                    let ai_config = config.ai.as_ref().cloned()
                        .unwrap_or_default();
                    println!("  AI provider: {}", ai_config.provider);
                    let vault = sentinel_membrane::credentials::CredentialVault::new();
                    match sentinel_cortex::provider::create_provider(
                        config.ai.as_ref(),
                        &vault,
                    ) {
                        Ok(provider) => println!("  AI model: {} — OK", provider.model()),
                        Err(e) => println!("  AI credentials: MISSING ({e})"),
                    }

                    // Check database
                    let db_path = sentinel_memory::db::default_db_path();
                    println!("  Database: {}", db_path.display());
                    if db_path.exists() {
                        println!("  Database status: OK");
                    } else {
                        println!("  Database status: will be created on first run");
                    }

                    Ok(())
                }
                Err(e) => {
                    eprintln!("Configuration error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Command::Travel { action } => {
            let pool = open_db().await?;
            let state = sentinel_memory::state::StateManager::new(pool);
            match action {
                commands::TravelAction::Set { destination, from, to, hotel, timezone } => {
                    let mode = sentinel_core::types::TravelMode {
                        destination: destination.clone(),
                        hotel,
                        start_date: from,
                        end_date: to,
                        timezone_override: timezone,
                        weather_lat: None,
                        weather_lon: None,
                        active: true,
                    };
                    state.set_travel_mode(&mode).await?;
                    println!("Travel mode activated: {}", mode.summary());
                }
                commands::TravelAction::Clear => {
                    state.clear_travel_mode().await?;
                    println!("Travel mode cleared. Welcome back!");
                }
                commands::TravelAction::Status => {
                    match state.get_travel_mode().await? {
                        Some(mode) => println!("{}", mode.summary()),
                        None => println!("Not in travel mode."),
                    }
                }
            }
            Ok(())
        }
        Command::TestEvent { kind } => {
            let config = load_config(cli.config.as_deref())?;
            let pool = open_db().await?;
            daemon::run_test_event(config, pool, &kind).await
        }
        Command::Household { action } => {
            handle_household(cli.config.as_deref(), action).await
        }
        Command::Memory { action } => {
            handle_memory(action).await
        }
        Command::Ledger { action } => {
            handle_ledger(action).await
        }
        Command::Cost { month } => {
            handle_cost(month).await
        }
        Command::Export { format, output } => {
            handle_export(&format, output.as_deref()).await
        }
        Command::Reset { confirm } => {
            handle_reset(confirm).await
        }
        Command::ValidateData { path, r#type } => {
            handle_validate_data(&path, &r#type)
        }
        Command::Tasks { action } => {
            handle_tasks(action).await
        }
    }
}

async fn handle_memory(action: commands::MemoryAction) -> Result<()> {
    use commands::MemoryAction;
    let pool = open_db().await?;
    let state = sentinel_memory::state::StateManager::new(pool);

    match action {
        MemoryAction::List => {
            let memories = state.get_memories().await?;
            if memories.is_empty() {
                println!("No memories stored.");
            } else {
                for m in &memories {
                    let tags = if m.tags.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", m.tags.join(", "))
                    };
                    println!("{} {}{tags}", m.id, m.content);
                }
                println!("\n{} memories total.", memories.len());
            }
        }
        MemoryAction::Search { query } => {
            let results = state.search_memories(&query, 50).await?;
            if results.is_empty() {
                println!("No memories matching \"{query}\".");
            } else {
                for m in &results {
                    println!("{} {}", m.id, m.content);
                }
            }
        }
        MemoryAction::Delete { id } => {
            if state.delete_memory(&id).await? {
                println!("Memory {id} deleted.");
            } else {
                println!("Memory {id} not found.");
            }
        }
        MemoryAction::DeleteTag { tag } => {
            let count = state.delete_memories_by_tag(&tag).await?;
            println!("Deleted {count} memories with tag \"{tag}\".");
        }
        MemoryAction::DeleteBefore { days } => {
            let count = state.delete_memories_before(days).await?;
            println!("Deleted {count} memories older than {days} days.");
        }
        MemoryAction::Stats => {
            let count = state.count_memories().await?;
            let obs = state.get_recent_observations(10000).await?;
            println!("Memories: {count}");
            println!("Observations: {}", obs.len());
        }
    }
    Ok(())
}

async fn handle_ledger(action: commands::LedgerAction) -> Result<()> {
    use commands::LedgerAction;
    let pool = open_db().await?;
    let ledger = sentinel_memory::ledger::Ledger::new(pool);

    match action {
        LedgerAction::Recent { limit } => {
            let entries = ledger.recent(limit).await?;
            if entries.is_empty() {
                println!("Ledger is empty.");
            } else {
                for e in &entries {
                    println!("[{}] {} — {}", e.timestamp.format("%Y-%m-%d %H:%M"), e.category, e.content);
                }
            }
        }
        LedgerAction::Search { query, limit } => {
            let results = ledger.search(&query, limit).await?;
            if results.is_empty() {
                println!("No ledger entries matching \"{query}\".");
            } else {
                for e in &results {
                    println!("[{}] {} — {}", e.timestamp.format("%Y-%m-%d %H:%M"), e.category, e.content);
                }
            }
        }
        LedgerAction::Stats => {
            let total = ledger.count().await?;
            println!("Total ledger entries: {total}");
            let categories = [
                sentinel_memory::ledger::LedgerCategory::EmailReceived,
                sentinel_memory::ledger::LedgerCategory::TaskCompleted,
                sentinel_memory::ledger::LedgerCategory::MealCooked,
                sentinel_memory::ledger::LedgerCategory::UserNote,
                sentinel_memory::ledger::LedgerCategory::DepartureAlert,
            ];
            for cat in &categories {
                let count = ledger.count_by_category(cat).await?;
                if count > 0 {
                    println!("  {cat}: {count}");
                }
            }
        }
        LedgerAction::Purge { older_than } => {
            let count = ledger.purge_older_than(older_than).await?;
            println!("Purged {count} ledger entries older than {older_than} days.");
        }
    }
    Ok(())
}

async fn handle_cost(month: Option<String>) -> Result<()> {
    let pool = open_db().await?;
    let audit = sentinel_membrane::audit::AuditLog::new(pool);

    // Parse month or use current
    let (year, mon) = if let Some(ref m) = month {
        let parts: Vec<&str> = m.split('-').collect();
        if parts.len() != 2 {
            anyhow::bail!("Invalid month format. Use YYYY-MM.");
        }
        (parts[0].parse::<i32>()?, parts[1].parse::<u32>()?)
    } else {
        let now = chrono::Utc::now();
        (now.year(), now.month())
    };

    use chrono::Datelike;
    let start = chrono::NaiveDate::from_ymd_opt(year, mon, 1)
        .ok_or_else(|| anyhow::anyhow!("Invalid date"))?
        .and_hms_opt(0, 0, 0).unwrap()
        .and_utc();

    let cost = audit.total_cost_since(start).await?;
    let eur = cost.estimated_cost_eur();

    println!("AI Cost — {year}-{mon:02}");
    println!("  Input tokens:  {}", cost.input_tokens);
    println!("  Output tokens: {}", cost.output_tokens);
    println!("  Cached tokens: {}", cost.cached_tokens);
    println!("  Estimated cost: €{eur:.4}");

    // Budget check
    // Read config if available for spending limits
    if let Ok(config) = load_config(None) {
        if let Some(ref spending) = config.policy.spending {
            let pct = (eur / spending.monthly_ai_budget_euros) * 100.0;
            println!("  Budget: €{:.2}/month ({pct:.1}% used)", spending.monthly_ai_budget_euros);
            if pct >= spending.warn_at_percentage as f64 {
                println!("  ⚠ Warning: approaching budget limit!");
            }
        }
    }
    Ok(())
}

async fn handle_export(format: &str, output: Option<&str>) -> Result<()> {
    if format != "json" {
        anyhow::bail!("Only 'json' format is currently supported.");
    }

    let pool = open_db().await?;
    let state = sentinel_memory::state::StateManager::new(pool.clone());
    let ledger = sentinel_memory::ledger::Ledger::new(pool.clone());
    let audit = sentinel_membrane::audit::AuditLog::new(pool);

    let state_data = state.export_all().await?;
    let ledger_entries = ledger.recent(10000).await?;
    let audit_entries = audit.recent(10000).await?;

    let export = serde_json::json!({
        "exported_at": chrono::Utc::now().to_rfc3339(),
        "state": state_data,
        "ledger_entries": ledger_entries.len(),
        "audit_entries": audit_entries.len(),
    });

    let json = serde_json::to_string_pretty(&export)?;

    match output {
        Some(path) => {
            std::fs::write(path, &json)?;
            println!("Data exported to {path}");
        }
        None => println!("{json}"),
    }
    Ok(())
}

async fn handle_reset(confirm: bool) -> Result<()> {
    if !confirm {
        println!("This will delete ALL Sentinel data (memories, ledger, observations).");
        println!("Run with --confirm to proceed.");
        return Ok(());
    }

    let db_path = sentinel_memory::db::default_db_path();
    if db_path.exists() {
        std::fs::remove_file(&db_path)?;
        println!("Database deleted: {}", db_path.display());
    } else {
        println!("No database found at {}", db_path.display());
    }
    println!("Reset complete. Database will be recreated on next run.");
    Ok(())
}

async fn handle_household(
    config_path: Option<&std::path::Path>,
    action: commands::HouseholdAction,
) -> Result<()> {
    use commands::{HouseholdAction, HouseholdSetupKind};

    match action {
        HouseholdAction::Init => {
            let config = load_config(config_path)?;
            let hh = config.household.as_ref()
                .ok_or_else(|| anyhow::anyhow!("[household] section missing from config"))?;
            // Create the shared database (migrations run automatically)
            let _pool = open_shared_db(&hh.shared_db_path).await?;
            println!("Household database initialised at {}", hh.shared_db_path);
            Ok(())
        }
        HouseholdAction::AddMember { name } => {
            let config_path_resolved = SentinelConfig::resolve_path(config_path);
            let raw = std::fs::read_to_string(&config_path_resolved)?;

            // Check if [household] section exists
            if !raw.contains("[household]") {
                anyhow::bail!(
                    "No [household] section in config. Add one first, e.g.:\n\
                     [household]\n\
                     shared_db_path = \"/home/sentinel/household/shared.db\""
                );
            }

            // Append member to the TOML config
            let member_entry = format!(
                "\n[[household.members]]\nname = \"{name}\"\nuser_id = \"{name}\"\n"
            );
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&config_path_resolved)?;
            std::io::Write::write_all(&mut file, member_entry.as_bytes())?;
            println!("Added household member: {name}");
            Ok(())
        }
        HouseholdAction::Setup { kind } => {
            let config_path_resolved = SentinelConfig::resolve_path(config_path);
            let raw = std::fs::read_to_string(&config_path_resolved)?;
            if !raw.contains("[household]") {
                anyhow::bail!("Run `sentinel household init` first");
            }
            match kind {
                HouseholdSetupKind::Shopping { provider } => {
                    let line = format!(
                        "\n# Shared shopping provider\n# shopping_provider = \"{provider}\"\n"
                    );
                    println!("Shopping provider set to: {provider}");
                    println!("Add to your config:\n{line}");
                }
                HouseholdSetupKind::Calendar { family_caldav_url } => {
                    println!("Family calendar URL: {family_caldav_url}");
                    println!(
                        "Add to your [household] config:\n\
                         family_calendar_url = \"{family_caldav_url}\""
                    );
                }
            }
            Ok(())
        }
        HouseholdAction::Status => {
            let config = load_config(config_path)?;
            match config.household {
                Some(hh) => {
                    println!("Household: configured");
                    println!("  Shared DB: {}", hh.shared_db_path);
                    println!("  Shopping provider: {}", hh.shopping_provider);
                    println!("  Members: {}", if hh.members.is_empty() {
                        "none".to_string()
                    } else {
                        hh.members.iter().map(|m| m.name.as_str()).collect::<Vec<_>>().join(", ")
                    });
                    if let Some(url) = &hh.family_calendar_url {
                        println!("  Family calendar: {url}");
                    }
                    // Try opening the shared DB for live status
                    match open_shared_db(&hh.shared_db_path).await {
                        Ok(pool) => {
                            let user_id = config.user.name.to_lowercase();
                            let store = sentinel_memory::household::HouseholdStore::new(pool, user_id);
                            if let Ok(items) = store.shopping_list().await {
                                println!("  Shopping list items: {}", items.len());
                            }
                            if let Ok(meals) = store.todays_meals().await {
                                println!("  Today's meals planned: {}", meals.len());
                            }
                        }
                        Err(_) => println!("  Shared DB: not yet initialised"),
                    }
                }
                None => println!("Household not configured. Add [household] to your config."),
            }
            Ok(())
        }
    }
}

async fn open_db() -> Result<sqlx::SqlitePool> {
    let db_path = sentinel_memory::db::default_db_path();
    tracing::info!(path = %db_path.display(), "opening database");
    let pool = sentinel_memory::db::open(&db_path).await?;
    Ok(pool)
}

pub async fn open_shared_db(path: &str) -> Result<sqlx::SqlitePool> {
    let db_path = std::path::PathBuf::from(path);
    tracing::info!(path = %db_path.display(), "opening shared household database");
    let pool = sentinel_memory::db::open(&db_path).await?;
    Ok(pool)
}

fn load_config(explicit: Option<&std::path::Path>) -> Result<SentinelConfig> {
    let path = SentinelConfig::resolve_path(explicit);
    if !path.exists() {
        anyhow::bail!(
            "Config file not found: {}\n\
             Create one from config/sentinel.example.toml or set SENTINEL_CONFIG",
            path.display()
        );
    }
    Ok(SentinelConfig::load(&path)?)
}

async fn handle_tasks(action: commands::TasksAction) -> Result<()> {
    use commands::TasksAction;
    let pool = open_db().await?;
    let store = sentinel_memory::tasks::TaskStore::new(pool);

    match action {
        TasksAction::List => {
            let tasks = store.list_active().await?;
            if tasks.is_empty() {
                println!("No active tasks.");
            } else {
                println!("{} active task(s):\n", tasks.len());
                for t in &tasks {
                    let due = t.next_trigger
                        .map(|dt| format!("  next: {}", dt.format("%Y-%m-%d %H:%M UTC")))
                        .unwrap_or_else(|| "  next: unscheduled".into());
                    let notes = t.notes.as_deref()
                        .map(|n| format!("  notes: {n}\n"))
                        .unwrap_or_default();
                    println!("[{}] {} ({:?})\n{due}\n{notes}", t.id, t.title, t.urgency);
                }
            }
        }
        TasksAction::Due => {
            let tasks = store.list_due().await?;
            if tasks.is_empty() {
                println!("No tasks currently due.");
            } else {
                println!("{} due/overdue task(s):\n", tasks.len());
                for t in &tasks {
                    let trigger_str = t.next_trigger
                        .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
                        .unwrap_or_else(|| "?".into());
                    println!("[{}] {} — was due {}", t.id, t.title, trigger_str);
                }
            }
        }
    }
    Ok(())
}

fn handle_validate_data(path: &str, data_type: &str) -> Result<()> {
    let file_path = std::path::Path::new(path);
    if !file_path.exists() {
        anyhow::bail!("File not found: {path}");
    }

    let content = std::fs::read_to_string(file_path)?;

    match data_type {
        "holiday" => {
            let file: sentinel_core::holidays::HolidayFile = toml::from_str(&content)
                .map_err(|e| anyhow::anyhow!("Invalid holiday TOML: {e}"))?;
            println!("✓ Valid holiday file for {} ({})", file.country.name, file.country.code);
            println!("  {} fixed holidays", file.fixed_holidays.len());
            println!("  {} easter-relative holidays", file.easter_relative.len());
            println!("  {} nth-weekday holidays", file.nth_weekday_holidays.len());
            println!("  {} regions", file.regions.len());
            for (key, region) in &file.regions {
                println!("    {key}: {} ({} holidays)", region.name, region.holidays.len());
            }
        }
        "sports" => {
            let season: sentinel_watchers::sports::SeasonData = toml::from_str(&content)
                .map_err(|e| anyhow::anyhow!("Invalid sports TOML: {e}"))?;
            println!("✓ Valid sports file: {} ({})", season.series.name, season.series.id);
            println!("  {} rounds", season.rounds.len());
            for round in &season.rounds {
                println!("    {} — {} sessions", round.name, round.sessions.len());
            }
        }
        other => {
            anyhow::bail!("Unknown data type: {other}. Use 'holiday' or 'sports'.");
        }
    }

    Ok(())
}
