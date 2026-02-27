use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "sentinel",
    about = "Sentinel — AI suggests, human decides.",
    version
)]
pub struct Cli {
    /// Path to configuration file
    #[arg(short, long, global = true)]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Start the Sentinel daemon
    Run {
        /// Start the web dashboard on this port
        #[arg(long)]
        dashboard_port: Option<u16>,
    },

    /// Check configuration and credentials
    Check,

    /// Run the onboarding conversation via Signal
    Onboard,

    /// Manage travel mode
    Travel {
        #[command(subcommand)]
        action: TravelAction,
    },

    /// Send a test event through the pipeline (Phase 1 development)
    TestEvent {
        /// Type of test event: morning-briefing, email, signal
        #[arg(default_value = "morning-briefing")]
        kind: String,
    },

    /// Manage household (multi-user shared surface)
    Household {
        #[command(subcommand)]
        action: HouseholdAction,
    },

    /// Manage memories
    Memory {
        #[command(subcommand)]
        action: MemoryAction,
    },

    /// Manage the activity ledger
    Ledger {
        #[command(subcommand)]
        action: LedgerAction,
    },

    /// View AI cost tracking
    Cost {
        /// Show costs for a specific month (YYYY-MM), default: current month
        #[arg(long)]
        month: Option<String>,
    },

    /// Export all user data (GDPR)
    Export {
        /// Output format
        #[arg(long, default_value = "json")]
        format: String,
        /// Output file path
        #[arg(long, short)]
        output: Option<String>,
    },

    /// Reset all data (requires confirmation)
    Reset {
        /// Confirm reset (required)
        #[arg(long)]
        confirm: bool,
    },

    /// Validate community data files (holidays, sports)
    ValidateData {
        /// Path to a TOML file or directory to validate
        path: String,
        /// Data type: "holiday" or "sports"
        #[arg(long, short = 't')]
        r#type: String,
    },
}

#[derive(Subcommand)]
pub enum TravelAction {
    /// Activate travel mode
    Set {
        /// Destination city/region
        destination: String,
        /// Start date (YYYY-MM-DD)
        #[arg(long)]
        from: String,
        /// End date (YYYY-MM-DD)
        #[arg(long)]
        to: String,
        /// Hotel/accommodation name
        #[arg(long)]
        hotel: Option<String>,
        /// Timezone override (e.g. "Europe/London")
        #[arg(long)]
        timezone: Option<String>,
    },
    /// Deactivate travel mode
    Clear,
    /// Show current travel mode status
    Status,
}

#[derive(Subcommand)]
pub enum HouseholdAction {
    /// Initialise shared household database
    Init,
    /// Add a member to the household
    AddMember {
        /// Member name (lowercase unix username)
        name: String,
    },
    /// Configure a shared integration
    Setup {
        #[command(subcommand)]
        kind: HouseholdSetupKind,
    },
    /// Show household status
    Status,
}

#[derive(Subcommand)]
pub enum HouseholdSetupKind {
    /// Configure shared shopping list provider
    Shopping {
        /// Provider name (e.g. "bring")
        #[arg(long, default_value = "bring")]
        provider: String,
    },
    /// Configure shared family calendar
    Calendar {
        /// CalDAV URL for the family calendar
        #[arg(long)]
        family_caldav_url: String,
    },
}

#[derive(Subcommand)]
pub enum MemoryAction {
    /// List all memories
    List,
    /// Search memories by keyword
    Search {
        /// Search query
        query: String,
    },
    /// Delete a memory by ID
    Delete {
        /// Memory ID
        id: String,
    },
    /// Delete memories by tag
    DeleteTag {
        /// Tag to match
        tag: String,
    },
    /// Delete memories older than N days
    DeleteBefore {
        /// Age in days
        days: u32,
    },
    /// Show memory statistics
    Stats,
}

#[derive(Subcommand)]
pub enum LedgerAction {
    /// Show recent ledger entries
    Recent {
        /// Number of entries to show
        #[arg(default_value = "20")]
        limit: u32,
    },
    /// Search ledger entries
    Search {
        /// Search query
        query: String,
        /// Maximum results
        #[arg(long, default_value = "20")]
        limit: u32,
    },
    /// Show ledger statistics
    Stats,
    /// Purge entries older than N days
    Purge {
        /// Age in days
        #[arg(long)]
        older_than: u32,
    },
}
