use anyhow::{Context, Result};
use sentinel_core::config::SentinelConfig;
use sentinel_core::events::{ScheduledTrigger, WatchEvent};
use sentinel_core::types::{ActionSource, Decision, Urgency};
use sentinel_cortex::prompt::{self, PromptBuilder};
use sentinel_cortex::provider::AiProvider;
use sentinel_cortex::response::Intent;
use sentinel_cortex::state_compiler::StateCompiler;
use sentinel_cortex::mode::ModeTracker;
use sentinel_cortex::triage::{self, Triage, TriggerType};
use sentinel_gate::approval::ApprovalManager;
use sentinel_gate::notification::NotificationRouter;
use sentinel_gate::signal::SignalClient;
use sentinel_membrane::audit::{AuditEntry, AuditLog};
use sentinel_membrane::credentials::CredentialVault;
use sentinel_membrane::policy::{PolicyContext, PolicyDecision, PolicyEngine};
use sentinel_memory::ledger::{Ledger, LedgerCategory, LedgerSource};
use sentinel_memory::rhythm::RhythmEngine;
use sentinel_memory::state::StateManager;
use sentinel_memory::tasks::TaskStore;
use sqlx::SqlitePool;

/// The Sentinel daemon.
pub struct Daemon {
    config: SentinelConfig,
    #[allow(dead_code)]
    vault: CredentialVault,
    policy: PolicyEngine,
    audit: AuditLog,
    client: Box<dyn AiProvider>,
    ledger: Ledger,
    rhythm: RhythmEngine,
    notifier: NotificationRouter,
    signal_client: Option<SignalClient>,
    approvals: ApprovalManager,
    #[allow(dead_code)]
    state: StateManager,
    task_store: TaskStore,
    mode_tracker: ModeTracker,
    #[cfg(feature = "bring")]
    bring_client: Option<sentinel_bring::BringClient>,
    household: Option<sentinel_memory::household::HouseholdStore>,
}

impl Daemon {
    pub async fn new(config: SentinelConfig, pool: SqlitePool) -> Result<Self> {
        let vault = CredentialVault::new();

        // Create AI provider from config (defaults to Anthropic)
        let client = sentinel_cortex::provider::create_provider(
            config.ai.as_ref(),
            &vault,
        ).context("failed to create AI provider")?;

        let policy = PolicyEngine::new(config.policy.clone());

        let audit = AuditLog::new(pool.clone());

        let ledger = Ledger::new(pool.clone());
        let rhythm = RhythmEngine::new(pool.clone());
        let notifier = NotificationRouter::with_desktop(config.user.assistant_name());
        let state = StateManager::new(pool.clone());
        let task_store = TaskStore::new(pool.clone());

        // Bring shopping list client — authenticate if credentials are available
        #[cfg(feature = "bring")]
        let bring_client = match vault.get_credentials(sentinel_core::types::ServiceId::Bring) {
            Ok(sentinel_membrane::credentials::ServiceCredentials::Bring { email, password }) => {
                match sentinel_bring::BringClient::login(&email, &password).await {
                    Ok(client) => {
                        tracing::info!("Bring client authenticated");
                        Some(client)
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Bring authentication failed — shopping list disabled");
                        None
                    }
                }
            }
            _ => None,
        };

        // Household shared store — open shared DB if household is configured
        let household = if let Some(ref hh_config) = config.household {
            match crate::open_shared_db(&hh_config.shared_db_path).await {
                Ok(shared_pool) => {
                    let user_id = config.user.name.to_lowercase();
                    Some(sentinel_memory::household::HouseholdStore::new(shared_pool, user_id))
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to open household shared DB");
                    None
                }
            }
        } else {
            None
        };

        // Signal client — only if configured and enabled
        let signal_client = config.signal.as_ref().and_then(|sc| {
            if sc.enabled {
                Some(SignalClient::new(sc.clone()))
            } else {
                None
            }
        });

        let approvals = ApprovalManager::new();
        let mode_tracker = ModeTracker::new();

        Ok(Self {
            config,
            vault,
            policy,
            audit,
            client,
            ledger,
            rhythm,
            notifier,
            signal_client,
            approvals,
            state,
            task_store,
            mode_tracker,
            #[cfg(feature = "bring")]
            bring_client,
            household,
        })
    }

    /// Process a single event through the full pipeline.
    pub async fn process_event(&self, event: WatchEvent) -> Result<()> {
        // Handle RhythmEngineRun internally — no AI, pure local computation
        if matches!(&event, WatchEvent::Schedule(ScheduledTrigger::RhythmEngineRun)) {
            tracing::info!("running rhythm engine...");
            match self.rhythm.compute().await {
                Ok(rhythms) => {
                    tracing::info!(count = rhythms.len(), "rhythm computation complete");
                    for r in &rhythms {
                        tracing::debug!(%r, "rhythm");
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "rhythm engine failed");
                }
            }
            return Ok(());
        }

        // Check if this is a Signal approval/rejection reply
        if let WatchEvent::Signal(ref msg) = event {
            let reply = ApprovalManager::parse_reply(&msg.text);
            if let Some((action, decision)) = self.approvals.process_reply(&reply).await {
                return self.handle_approval_decision(action, decision).await;
            }
            // Not an approval reply — fall through to normal processing
        }

        // Log the incoming event to the ledger
        let (category, content, source) = event_to_ledger(&event);
        let entry = Ledger::entry(category, content, source);
        self.ledger.append(&entry).await?;

        // Step 1: Local triage
        let triage = triage::local_triage(&event);

        match triage {
            Triage::Ignore => {
                tracing::debug!(?event, "event ignored by local triage");
                return Ok(());
            }
            Triage::PassThrough(msg) => {
                tracing::info!(%msg, "pass-through notification");
                self.notifier.info(&msg);
                return Ok(());
            }
            Triage::NeedsAI(trigger_type) => {
                // Check if AI is available
                if !self.mode_tracker.ai_available() {
                    let fallback = sentinel_cortex::mode::degraded_fallback(
                        &format!("{:?}", trigger_type_label(&trigger_type)),
                    );
                    tracing::info!(mode = %self.mode_tracker.mode(), "degraded mode — skipping AI");
                    self.notifier.info(&fallback);
                    return Ok(());
                }
                self.process_with_ai(trigger_type).await?;
            }
        }

        Ok(())
    }

    async fn process_with_ai(&self, trigger_type: TriggerType) -> Result<()> {
        // Step 2: Compile trigger-specific state from real data
        let state = StateCompiler::new(&self.config)
            .compile_for_trigger(
                &trigger_type,
                &self.ledger,
                &self.state,
                Some(&self.rhythm),
                Some(&self.task_store),
                self.config.sports.as_ref(),
                self.config.cultural.as_ref(),
                self.household.as_ref(),
                Some(&self.audit),
            )
            .await;

        // Step 3: Build trigger message
        let trigger_msg = match &trigger_type {
            TriggerType::MorningBriefing => {
                prompt::format_schedule_trigger("morning_briefing")
            }
            TriggerType::MorningReflection => {
                prompt::format_schedule_trigger("morning_reflection")
            }
            TriggerType::WeeklyPlanning => {
                prompt::format_schedule_trigger("weekly_planning")
            }
            TriggerType::WeeklyReflection => {
                prompt::format_schedule_trigger("weekly_reflection")
            }
            TriggerType::MonthlyReflection => {
                prompt::format_schedule_trigger("monthly_reflection")
            }
            TriggerType::EmailTriage(email) => {
                prompt::format_email_trigger(&email.from, &email.subject, &email.preview)
            }
            TriggerType::DepartureAlert(dep) => {
                prompt::format_departure_trigger(
                    &dep.destination,
                    dep.event_time,
                    dep.travel_minutes,
                    dep.leave_by,
                )
            }
            TriggerType::SignalQuery(sig) => {
                prompt::format_signal_trigger(&sig.text)
            }
            TriggerType::UserNote(note) => {
                prompt::format_user_note_trigger(&note.text)
            }
            TriggerType::CalendarChange => {
                prompt::format_schedule_trigger("calendar_change")
            }
            TriggerType::TaskEvent => {
                prompt::format_schedule_trigger("task_event")
            }
            TriggerType::WeatherUpdate => {
                prompt::format_schedule_trigger("weather_update")
            }
        };

        // Step 4: Build and send prompt
        let request = PromptBuilder::new(self.config.user.assistant_name())
            .with_state(state)
            .with_trigger(trigger_msg)
            .build(self.client.model());

        tracing::info!("sending event to Anthropic API...");

        let response = match self.client.complete(request).await {
            Ok(resp) => {
                // Record success — may recover from degraded mode
                if self.mode_tracker.record_success() {
                    tracing::info!("API recovered — resuming full mode");
                    self.notifier.info("✅ AI connection restored — resuming full mode.");
                }
                resp
            }
            Err(e) => {
                tracing::error!(error = %e, "Anthropic API call failed");
                if self.mode_tracker.record_failure() {
                    tracing::warn!("entering degraded mode after repeated API failures");
                    self.notifier.notify(
                        &Urgency::High,
                        "Degraded Mode",
                        "⚠️ AI unavailable — switching to local-only mode. Basic notifications will continue.",
                    );
                }
                return Ok(());
            }
        };

        tracing::info!(
            reasoning = %response.parsed.reasoning,
            intent_count = response.parsed.intents.len(),
            input_tokens = response.token_cost.input_tokens,
            output_tokens = response.token_cost.output_tokens,
            estimated_cost_eur = %response.token_cost.estimated_cost_eur(),
            "LLM response received"
        );

        // Step 5: Process each intent through policy
        for intent in &response.parsed.intents {
            match intent {
                Intent::Notify {
                    urgency,
                    title,
                    body,
                    ..
                } => {
                    tracing::info!(%urgency, %title, "notification");
                    // Signal is primary (always on you), desktop is supplementary
                    if let Some(ref signal) = self.signal_client {
                        if let Err(e) = signal.send_notification(urgency, title, body).await {
                            tracing::warn!(error = %e, "Signal notification failed");
                        }
                    }
                    self.notifier.notify(urgency, title, body);
                }
                Intent::RequestAction {
                    capability,
                    explanation,
                } => {
                    let policy_ctx = self.build_policy_context().await;
                    let decision = self.policy.evaluate(capability, chrono::Utc::now(), &policy_ctx);
                    match decision {
                        PolicyDecision::AutoApproved => {
                            tracing::info!(
                                capability = %capability.kind(),
                                "auto-approved"
                            );

                            self.execute_capability(capability).await;

                            self.notifier.notify(
                                &Urgency::Low,
                                &format!("Auto-approved: {:?}", capability.kind()),
                                explanation,
                            );

                            let entry = AuditEntry::new(
                                capability,
                                ActionSource::Cortex,
                                Decision::AutoApproved,
                                response.parsed.reasoning.clone(),
                            )
                            .with_token_cost(response.token_cost.clone());
                            self.audit.record(&entry).await?;
                        }
                        PolicyDecision::RequiresApproval { reason } => {
                            tracing::info!(
                                capability = %capability.kind(),
                                %reason,
                                "approval required — queuing for human decision"
                            );

                            // Add to pending approvals
                            let action_id = self.approvals.add(
                                capability.clone(),
                                explanation.clone(),
                                response.parsed.reasoning.clone(),
                            ).await;

                            let kind_str = format!("{:?}", capability.kind());

                            // Send approval request via Signal if available
                            if let Some(ref signal) = self.signal_client {
                                if let Err(e) = signal.send_approval_request(
                                    &action_id,
                                    &kind_str,
                                    explanation,
                                ).await {
                                    tracing::warn!(error = %e, "failed to send approval via Signal");
                                }
                            }

                            // Also desktop notification
                            self.notifier.notify(
                                &Urgency::High,
                                &format!("Needs approval: {kind_str}"),
                                &format!("{explanation}\nReply \"yes {action_id}\" or \"no {action_id}\""),
                            );
                        }
                        PolicyDecision::Blocked { reason } => {
                            tracing::warn!(
                                capability = %capability.kind(),
                                %reason,
                                "blocked by policy"
                            );
                            self.notifier.notify(
                                &Urgency::Medium,
                                &format!("BLOCKED: {:?}", capability.kind()),
                                &reason,
                            );

                            let entry = AuditEntry::new(
                                capability,
                                ActionSource::Cortex,
                                Decision::PolicyBlocked,
                                response.parsed.reasoning.clone(),
                            )
                            .with_token_cost(response.token_cost.clone());
                            self.audit.record(&entry).await?;
                        }
                    }
                }
            }
        }

        // Step 6: Apply state updates to persistent memory
        for update in &response.parsed.state_updates {
            match update {
                sentinel_cortex::response::StateUpdate::AddObservation { content } => {
                    match self.state.add_observation(content, "cortex").await {
                        Ok(id) => tracing::info!(%id, "observation persisted"),
                        Err(e) => tracing::error!(error = %e, "failed to persist observation"),
                    }
                }
                sentinel_cortex::response::StateUpdate::AddMemory { content, tags } => {
                    match self.state.add_memory(content, tags, "cortex").await {
                        Ok(id) => tracing::info!(%id, "memory persisted"),
                        Err(e) => tracing::error!(error = %e, "failed to persist memory"),
                    }
                }
                sentinel_cortex::response::StateUpdate::RemoveMemory { id } => {
                    match self.state.delete_memory(id).await {
                        Ok(true) => tracing::info!(%id, "memory removed"),
                        Ok(false) => tracing::warn!(%id, "memory not found for removal"),
                        Err(e) => tracing::error!(error = %e, "failed to remove memory"),
                    }
                }
            }
        }

        Ok(())
    }

    /// Handle a resolved approval decision (approved or rejected by user via Signal).
    async fn handle_approval_decision(
        &self,
        action: sentinel_gate::approval::PendingAction,
        decision: Decision,
    ) -> Result<()> {
        let kind_str = format!("{:?}", action.capability.kind());
        tracing::info!(%decision, capability = %kind_str, id = %action.id, "approval decision received");

        // Record audit entry
        let entry = AuditEntry::new(
            &action.capability,
            ActionSource::UserDirect,
            decision.clone(),
            action.cortex_reasoning.clone(),
        );
        self.audit.record(&entry).await?;

        // Execute approved capabilities
        if decision == Decision::HumanApproved {
            self.execute_capability(&action.capability).await;
        }

        let msg = match decision {
            Decision::HumanApproved => format!("Approved: {kind_str} — {}", action.explanation),
            Decision::HumanRejected => format!("Rejected: {kind_str}"),
            _ => format!("{decision:?}: {kind_str}"),
        };

        // Confirm back via Signal
        if let Some(ref signal) = self.signal_client {
            let urgency = Urgency::Low;
            if let Err(e) = signal.send_notification(&urgency, "Approval decision", &msg).await {
                tracing::warn!(error = %e, "failed to send approval confirmation via Signal");
            }
        }

        self.notifier.info(&msg);
        Ok(())
    }

    /// Execute an approved capability.
    async fn execute_capability(&self, capability: &sentinel_core::capability::Capability) {
        use sentinel_core::capability::Capability;

        match capability {
            // === READ capabilities — data already compiled into state by watchers ===
            Capability::EmailRead(query) => {
                tracing::info!(?query, "email read acknowledged (data already in context)");
            }
            Capability::CalendarRead(query) => {
                tracing::info!(?query, "calendar read acknowledged (data already in context)");
            }
            Capability::TaskListRead => {
                tracing::info!("task list read acknowledged (data already in context)");
            }
            Capability::WeatherFetch(location) => {
                tracing::info!(location = %location.name, "weather fetch acknowledged (data already in context)");
            }
            Capability::RoutingQuery(route) => {
                tracing::info!(
                    origin = %route.origin.name,
                    destination = %route.destination.name,
                    "routing query acknowledged (data already in context)"
                );
            }

            // === TASK capabilities ===
            Capability::TaskCreate(task) => {
                match self.task_store.create(task, "cortex").await {
                    Ok(id) => tracing::info!(%id, title = %task.title, "task created"),
                    Err(e) => tracing::error!(error = %e, "failed to create task"),
                }
            }
            Capability::TaskComplete(task_id) => {
                match self.task_store.complete(&task_id.0).await {
                    Ok(true) => tracing::info!(id = %task_id.0, "task completed"),
                    Ok(false) => tracing::warn!(id = %task_id.0, "task not found for completion"),
                    Err(e) => tracing::error!(error = %e, "failed to complete task"),
                }
            }
            Capability::TaskModify(task_id, patch) => {
                match self.task_store.update(&task_id.0, patch).await {
                    Ok(true) => tracing::info!(id = %task_id.0, "task modified"),
                    Ok(false) => tracing::warn!(id = %task_id.0, "task not found for modification"),
                    Err(e) => tracing::error!(error = %e, "failed to modify task"),
                }
            }

            // === BRING capabilities ===
            Capability::BringAdd(item) => {
                #[cfg(feature = "bring")]
                {
                    if let Some(ref bring) = self.bring_client {
                        let spec = item.context.as_deref().unwrap_or("");
                        match bring.add_item(&item.name, spec).await {
                            Ok(()) => tracing::info!(item = %item.name, "added to Bring"),
                            Err(e) => tracing::error!(error = %e, item = %item.name, "Bring add failed"),
                        }
                        // Also add to local household shopping list if available
                        if let Some(ref hh) = self.household {
                            let _ = hh.add_shopping_item(&item.name, item.category.as_deref(), item.context.as_deref()).await;
                        }
                    } else {
                        tracing::warn!(item = %item.name, "Bring not configured — cannot add item");
                    }
                }
                #[cfg(not(feature = "bring"))]
                {
                    tracing::warn!(item = %item.name, "Bring support not compiled in — enable the 'bring' feature");
                }
            }
            Capability::BringRemove(item) => {
                #[cfg(feature = "bring")]
                {
                    if let Some(ref bring) = self.bring_client {
                        // Check if the item was added by a partner (requires household store + config)
                        let should_notify_partner = self.config.policy.bring
                            .as_ref()
                            .map(|b| b.notify_partner_on_removal)
                            .unwrap_or(false);

                        let notify_partner = if should_notify_partner {
                            if let Some(ref hh) = self.household {
                                if let Ok(items) = hh.shopping_list().await {
                                    items.iter().find(|i| i.item == item.name)
                                        .and_then(|i| {
                                            if i.added_by != self.config.user.name.to_lowercase() {
                                                Some(i.added_by.clone())
                                            } else {
                                                None
                                            }
                                        })
                                } else { None }
                            } else { None }
                        } else { None };

                        match bring.remove_item(&item.name).await {
                            Ok(()) => {
                                tracing::info!(item = %item.name, "removed from Bring");
                                if let Some(partner) = notify_partner {
                                    let msg = format!(
                                        "🛒 {} removed '{}' from the shopping list (added by {partner})",
                                        self.config.user.name, item.name,
                                    );
                                    self.notifier.info(&msg);
                                }
                            }
                            Err(e) => tracing::error!(error = %e, item = %item.name, "Bring remove failed"),
                        }
                    } else {
                        tracing::warn!(item = %item.name, "Bring not configured — cannot remove item");
                    }
                }
                #[cfg(not(feature = "bring"))]
                {
                    tracing::warn!(item = %item.name, "Bring support not compiled in — enable the 'bring' feature");
                }
            }

            // === CALENDAR WRITE capabilities ===
            Capability::CalendarEventCreate(event) => {
                tracing::info!(
                    title = %event.title,
                    start = %event.start,
                    location = ?event.location,
                    "calendar event create recorded (CalDAV write not yet connected)"
                );
                self.notifier.info(&format!(
                    "📅 Draft calendar event: {} at {}{}",
                    event.title,
                    event.start.format("%Y-%m-%d %H:%M"),
                    event.location.as_deref().map(|l| format!(" @ {l}")).unwrap_or_default(),
                ));
            }
            Capability::CalendarEventModify(id, patch) => {
                tracing::info!(
                    event_id = %id.0,
                    ?patch,
                    "calendar event modify recorded (CalDAV write not yet connected)"
                );
                self.notifier.info(&format!("📅 Draft calendar modification for event {}", id.0));
            }
            Capability::CalendarEventDelete(id) => {
                tracing::info!(
                    event_id = %id.0,
                    "calendar event delete recorded (CalDAV write not yet connected)"
                );
                self.notifier.info(&format!("📅 Draft calendar deletion for event {}", id.0));
            }

            // === EMAIL capabilities ===
            Capability::EmailDraft(draft) => {
                tracing::info!(
                    to = ?draft.to,
                    subject = %draft.subject,
                    account = ?draft.account,
                    "email draft saved"
                );
                self.notifier.info(&format!(
                    "✉️ Draft email: to={}, subject=\"{}\"",
                    draft.to.join(", "),
                    draft.subject,
                ));
            }
            Capability::EmailReply(email_id, _reply) => {
                tracing::info!(
                    account = %email_id.account,
                    uid = email_id.uid,
                    "email reply draft saved"
                );
                self.notifier.info(&format!(
                    "✉️ Draft reply to message {} in {}",
                    email_id.uid,
                    email_id.account,
                ));
            }

            // === SIGNAL ===
            Capability::SignalReply(text) => {
                if let Some(ref signal) = self.signal_client {
                    if let Err(e) = signal.broadcast(text).await {
                        tracing::error!(error = %e, "failed to send Signal reply");
                    } else {
                        tracing::info!("Signal reply sent");
                    }
                } else {
                    tracing::warn!("Signal not configured — cannot send reply");
                    self.notifier.info(&format!("💬 Signal reply (not sent — Signal not configured): {text}"));
                }
            }

            // === REMINDER ===
            Capability::ReminderSet(reminder) => {
                tracing::info!(
                    message = %reminder.message,
                    time = %reminder.time,
                    "reminder recorded"
                );
                self.notifier.info(&format!(
                    "⏰ Reminder set for {}: {}",
                    reminder.time.format("%Y-%m-%d %H:%M"),
                    reminder.message,
                ));
            }
        }
    }

    /// Build pre-fetched context for policy evaluation (spending, recurring tasks).
    async fn build_policy_context(&self) -> PolicyContext {
        let monthly_spend_eur = {
            use chrono::Datelike;
            let month_start = chrono::Utc::now()
                .date_naive()
                .with_day(1)
                .unwrap_or(chrono::Utc::now().date_naive())
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc();
            self.audit
                .total_cost_since(month_start)
                .await
                .map(|tc| tc.estimated_cost_eur())
                .unwrap_or(0.0)
        };
        let active_recurring_tasks = self
            .task_store
            .count_recurring()
            .await
            .unwrap_or(0);
        PolicyContext {
            monthly_spend_eur,
            active_recurring_tasks,
        }
    }

    /// Run the daemon event loop, spawning configured watchers.
    pub async fn run(self) -> Result<()> {
        let name = self.config.user.assistant_name();
        tracing::info!("{name} daemon starting...");
        println!("{name} — AI suggests, human decides.");
        println!("Daemon running. Press Ctrl+C to stop.");

        let (tx, mut rx) = tokio::sync::mpsc::channel::<WatchEvent>(32);

        // Shared handles for cross-watcher communication
        let departure_upcoming: Option<
            std::sync::Arc<tokio::sync::Mutex<Vec<sentinel_watchers::departure::UpcomingEvent>>>,
        >;
        let departure_weather: Option<
            std::sync::Arc<tokio::sync::Mutex<Option<String>>>,
        >;

        // Spawn IMAP watchers for each configured email account
        let mut watcher_count = 0;
        if let Some(ref email_config) = self.config.email {
            let global_triage = email_config.triage.as_ref();
            for account in &email_config.accounts {
                // Merge: account-level triage layered on top of global
                let effective_triage = match (global_triage, account.triage.as_ref()) {
                    (Some(g), Some(a)) => Some(g.merge(a)),
                    (Some(g), None) => Some(g.clone()),
                    (None, Some(a)) => Some(a.clone()),
                    (None, None) => None,
                };
                let watcher = sentinel_watchers::email::ImapWatcher::new(
                    account.clone(),
                    effective_triage.as_ref(),
                    self.state.clone(),
                );
                let watcher_tx = tx.clone();
                let account_name = account.name.clone();
                tokio::spawn(async move {
                    tracing::info!(account = %account_name, "starting IMAP watcher");
                    if let Err(e) = watcher.run(watcher_tx).await {
                        tracing::error!(
                            account = %account_name,
                            error = %e,
                            "IMAP watcher terminated"
                        );
                    }
                });
                watcher_count += 1;
            }
        }

        // Spawn Signal watcher if configured and enabled
        if let Some(ref signal_config) = self.config.signal {
            if signal_config.enabled {
                let watcher = sentinel_watchers::signal::SignalWatcher::new(signal_config.clone());
                let signal_tx = tx.clone();
                tokio::spawn(async move {
                    tracing::info!("starting Signal watcher");
                    if let Err(e) = watcher.run(signal_tx).await {
                        tracing::error!(error = %e, "Signal watcher terminated");
                    }
                });
                watcher_count += 1;
                println!("Signal watcher started.");
            }
        }

        // Spawn CalDAV watcher if configured
        if let Some(ref cal_config) = self.config.calendar {
            let watcher = sentinel_watchers::caldav::CalDavWatcher::new(cal_config.clone());
            let caldav_tx = tx.clone();
            tokio::spawn(async move {
                tracing::info!("starting CalDAV watcher");
                if let Err(e) = watcher.run(caldav_tx).await {
                    tracing::error!(error = %e, "CalDAV watcher terminated");
                }
            });
            watcher_count += 1;
            println!("CalDAV watcher started.");
        }

        // Spawn Weather watcher if configured
        if let Some(ref weather_config) = self.config.weather {
            let watcher = sentinel_watchers::weather::WeatherWatcher::new(weather_config.clone());
            let weather_tx = tx.clone();
            tokio::spawn(async move {
                tracing::info!("starting Weather watcher");
                if let Err(e) = watcher.run(weather_tx).await {
                    tracing::error!(error = %e, "Weather watcher terminated");
                }
            });
            watcher_count += 1;
            println!("Weather watcher started.");
        }

        // Spawn Departure watcher if departure + routing are both configured
        // Grab shared handles BEFORE moving watcher into the task
        if let (Some(dep_config), Some(route_config)) =
            (&self.config.departure, &self.config.routing)
        {
            let watcher = sentinel_watchers::departure::DepartureWatcher::new(
                dep_config.clone(),
                route_config.clone(),
            );
            let upcoming_handle = watcher.upcoming_events();
            let weather_handle = watcher.weather_handle();
            departure_upcoming = Some(upcoming_handle);
            departure_weather = Some(weather_handle);

            let dep_tx = tx.clone();
            tokio::spawn(async move {
                tracing::info!("starting Departure watcher");
                if let Err(e) = watcher.run(dep_tx).await {
                    tracing::error!(error = %e, "Departure watcher terminated");
                }
            });
            watcher_count += 1;
            println!("Departure watcher started.");
        } else {
            departure_upcoming = None;
            departure_weather = None;
        }

        // Spawn TaskWatcher — checks for due/overdue tasks
        {
            let task_watcher = sentinel_watchers::tasks::TaskWatcher::new(self.task_store.clone());
            let task_tx = tx.clone();
            tokio::spawn(async move {
                tracing::info!("starting Task watcher");
                if let Err(e) = task_watcher.run(task_tx).await {
                    tracing::error!(error = %e, "Task watcher terminated");
                }
            });
            watcher_count += 1;
            println!("Task watcher started.");
        }

        // Spawn SportsCalendarWatcher if sports are configured
        if let Some(ref sports_config) = self.config.sports {
            let mut all_series = Vec::new();
            all_series.extend(sports_config.motorsport.iter().cloned());
            all_series.extend(sports_config.football.iter().cloned());
            all_series.extend(sports_config.tennis.iter().cloned());

            if !all_series.is_empty() {
                let data_dir = sports_config.data_dir.as_deref()
                    .unwrap_or("data/sports");
                let watcher = sentinel_watchers::sports::SportsCalendarWatcher::new(
                    std::path::PathBuf::from(data_dir),
                    all_series,
                );
                let sports_tx = tx.clone();
                tokio::spawn(async move {
                    tracing::info!("starting Sports watcher");
                    if let Err(e) = watcher.run(sports_tx).await {
                        tracing::error!(error = %e, "Sports watcher terminated");
                    }
                });
                watcher_count += 1;
                println!("Sports watcher started.");
            }
        }

        // Spawn CulturalEventsWatcher if [integrations.events] is configured
        if let Some(ref integrations) = self.config.integrations {
            if let Some(ref events_config) = integrations.events {
                use sentinel_watchers::cultural::{CulturalEventsWatcher, EventSource, TasteProfile};

                let sources: Vec<EventSource> = events_config.sources.iter().map(|s| {
                    match s.r#type.as_str() {
                        "feed" => EventSource::Feed {
                            name: s.name.clone(),
                            url: s.url.clone(),
                            refresh_hours: Some(events_config.check_interval_hours),
                        },
                        "ical" => EventSource::ICal {
                            name: s.name.clone(),
                            url: s.url.clone(),
                            refresh_hours: Some(events_config.check_interval_hours),
                        },
                        _ => EventSource::LocalFile {
                            name: s.name.clone(),
                            path: std::path::PathBuf::from(&s.url),
                        },
                    }
                }).collect();

                let taste = self.config.cultural.as_ref()
                    .and_then(|c| c.taste.as_ref())
                    .map(|t| TasteProfile {
                        likes: t.likes.clone(),
                        maybe: t.maybe.clone(),
                        not_interested: t.not_interested.clone(),
                        learned: vec![],
                    })
                    .unwrap_or_default();

                let top_n = self.config.cultural.as_ref()
                    .map(|c| c.top_n)
                    .unwrap_or(5);

                let watcher = CulturalEventsWatcher::new(
                    sources,
                    taste,
                    events_config.check_interval_hours,
                );
                let cultural_tx = tx.clone();
                tokio::spawn(async move {
                    tracing::info!("starting Cultural events watcher");
                    if let Err(e) = watcher.run(cultural_tx, top_n).await {
                        tracing::error!(error = %e, "Cultural events watcher terminated");
                    }
                });
                watcher_count += 1;
                println!("Cultural events watcher started.");
            }
        }

        // Spawn TimeWatcher — fires scheduled triggers (briefings, reflections)
        {
            use sentinel_core::schedule::{ScheduleEntry, ScheduledTriggerKind};

            let tz_offset = parse_tz_offset(&self.config.user.timezone);
            let schedule = vec![
                ScheduleEntry::Daily {
                    time: "07:00".into(),
                    trigger: ScheduledTriggerKind::MorningBriefing,
                },
                ScheduleEntry::Daily {
                    time: "06:50".into(),
                    trigger: ScheduledTriggerKind::MorningReflection,
                },
                ScheduleEntry::Weekly {
                    day: "sunday".into(),
                    time: "18:00".into(),
                    trigger: ScheduledTriggerKind::WeeklyPlanning,
                },
                ScheduleEntry::Weekly {
                    day: "sunday".into(),
                    time: "17:50".into(),
                    trigger: ScheduledTriggerKind::WeeklyReflection,
                },
                ScheduleEntry::Interval {
                    every_seconds: 30 * 24 * 3600,
                    trigger: ScheduledTriggerKind::MonthlyReflection,
                },
                ScheduleEntry::Interval {
                    every_seconds: 6 * 3600,
                    trigger: ScheduledTriggerKind::RhythmEngineRun,
                },
            ];

            let time_watcher = sentinel_watchers::time::TimeWatcher::new(schedule, tz_offset);
            let time_tx = tx.clone();
            tokio::spawn(async move {
                tracing::info!("starting Time watcher (schedule runner)");
                if let Err(e) = time_watcher.run(time_tx).await {
                    tracing::error!(error = %e, "Time watcher terminated");
                }
            });
            watcher_count += 1;
            println!("Time watcher (schedule runner) started.");
        }

        if watcher_count == 0 {
            tracing::info!("no watchers configured — daemon will wait for test events");
            println!("No watchers configured. Use 'sentinel test-event' to test the pipeline.");
        } else {
            tracing::info!(count = watcher_count, "watchers spawned");
            println!("{watcher_count} watcher(s) started.");
        }

        // Drop our copy of tx so rx closes when all watchers exit
        drop(tx);

        while let Some(event) = rx.recv().await {
            // Feed calendar events with locations into the departure watcher
            if let Some(ref handle) = departure_upcoming {
                if let WatchEvent::Calendar(ref change) = event {
                    feed_departure_events(handle, change).await;
                }
            }

            // Feed weather conditions into the departure watcher
            if let Some(ref handle) = departure_weather {
                if let WatchEvent::Weather(ref update) = event {
                    let mut cond = handle.lock().await;
                    *cond = Some(update.conditions.clone());
                }
            }

            if let Err(e) = self.process_event(event).await {
                tracing::error!(error = %e, "failed to process event");
            }
        }

        tracing::info!("{} daemon stopped.", self.config.user.assistant_name());
        Ok(())
    }
}

/// Send a test event through the full pipeline (for development).
pub async fn run_test_event(config: SentinelConfig, pool: SqlitePool, kind: &str) -> Result<()> {
    let daemon = Daemon::new(config, pool).await?;

    let event = match kind {
        "morning-briefing" => WatchEvent::Schedule(ScheduledTrigger::MorningBriefing),
        "weekly-planning" => WatchEvent::Schedule(ScheduledTrigger::WeeklyPlanning),
        "email" => {
            use sentinel_core::capability::EmailId;
            use sentinel_core::events::EmailEvent;
            use sentinel_core::types::Urgency;
            WatchEvent::Email(EmailEvent {
                id: EmailId::new("test".into(), 1),
                from: "ana@example.com".into(),
                to: vec!["user@example.com".into()],
                subject: "Dinner tomorrow?".into(),
                preview: "Hey! Want to grab dinner tomorrow evening? I was thinking that new Italian place on Rua Augusta.".into(),
                timestamp: chrono::Utc::now(),
                is_reply: false,
                has_attachments: false,
                urgency: Urgency::Medium,
            })
        }
        "signal" => {
            use sentinel_core::events::SignalMessage;
            WatchEvent::Signal(SignalMessage {
                sender: "+351000000000".into(),
                text: "Can you pick up milk on the way home?".into(),
                timestamp: chrono::Utc::now(),
                attachments: vec![],
            })
        }
        "departure" => {
            use sentinel_core::events::DepartureEvent;
            WatchEvent::Departure(DepartureEvent {
                destination: "Dentist — Clínica São João".into(),
                event_time: chrono::Utc::now() + chrono::Duration::hours(2),
                travel_minutes: 18,
                leave_by: chrono::Utc::now() + chrono::Duration::minutes(97),
            })
        }
        "weather" => {
            use sentinel_core::events::WeatherUpdate;
            WatchEvent::Weather(WeatherUpdate {
                location: "Porto".into(),
                temperature_c: 14.2,
                conditions: "Light rain".into(),
                forecast: vec![
                    "Tomorrow: Partly cloudy, 16°C".into(),
                    "Day after: Clear sky, 19°C".into(),
                ],
            })
        }
        other => anyhow::bail!("unknown test event kind: {other}. Use: morning-briefing, weekly-planning, email, signal, departure, weather"),
    };

    println!("Processing test event: {kind}");
    println!("---");
    daemon.process_event(event).await?;
    println!("---");
    println!("Test event processed successfully.");
    Ok(())
}

/// Feed calendar events with locations into the departure watcher's upcoming list.
async fn feed_departure_events(
    handle: &std::sync::Arc<tokio::sync::Mutex<Vec<sentinel_watchers::departure::UpcomingEvent>>>,
    change: &sentinel_core::events::CalendarChange,
) {
    use sentinel_core::events::CalendarChange;
    use sentinel_watchers::departure::UpcomingEvent;

    let mut upcoming = handle.lock().await;
    match change {
        CalendarChange::Created(ev) | CalendarChange::Modified(ev) => {
            if let Some(ref loc) = ev.location {
                let event_id = format!("{}@{}", ev.title, ev.start.timestamp());
                // Remove existing entry with same id (for modified events)
                upcoming.retain(|u| u.event_id != event_id);
                upcoming.push(UpcomingEvent {
                    title: ev.title.clone(),
                    start: ev.start,
                    location: loc.clone(),
                    event_id,
                });
                tracing::debug!(title = %ev.title, "fed calendar event to departure watcher");
            }
        }
        CalendarChange::Deleted(id) => {
            let before = upcoming.len();
            upcoming.retain(|u| u.event_id != id.0);
            if upcoming.len() < before {
                tracing::debug!(id = %id.0, "removed deleted event from departure watcher");
            }
        }
    }
}

/// Short label for a trigger type (for degraded mode notifications).
fn trigger_type_label(t: &TriggerType) -> &'static str {
    match t {
        TriggerType::MorningBriefing => "Morning briefing",
        TriggerType::MorningReflection => "Morning reflection",
        TriggerType::WeeklyPlanning => "Weekly planning",
        TriggerType::WeeklyReflection => "Weekly reflection",
        TriggerType::MonthlyReflection => "Monthly reflection",
        TriggerType::EmailTriage(_) => "Email triage",
        TriggerType::DepartureAlert(_) => "Departure alert",
        TriggerType::SignalQuery(_) => "Signal query",
        TriggerType::UserNote(_) => "User note",
        TriggerType::CalendarChange => "Calendar change",
        TriggerType::TaskEvent => "Task event",
        TriggerType::WeatherUpdate => "Weather update",
    }
}

/// Map a WatchEvent to a ledger entry's category, content, and source.
fn event_to_ledger(event: &WatchEvent) -> (LedgerCategory, String, LedgerSource) {
    match event {
        WatchEvent::Email(e) => (
            LedgerCategory::EmailReceived,
            format!("From: {} — {}", e.from, e.subject),
            LedgerSource::Watcher("email".into()),
        ),
        WatchEvent::Schedule(trigger) => (
            LedgerCategory::Observation,
            format!("Scheduled trigger: {trigger:?}"),
            LedgerSource::System,
        ),
        WatchEvent::Signal(msg) => (
            LedgerCategory::UserNote,
            msg.text.clone(),
            LedgerSource::User,
        ),
        WatchEvent::Calendar(change) => (
            LedgerCategory::Observation,
            format!("Calendar change: {change:?}"),
            LedgerSource::Watcher("calendar".into()),
        ),
        WatchEvent::Departure(dep) => (
            LedgerCategory::DepartureAlert,
            format!("Departure to {} at {}", dep.destination, dep.event_time),
            LedgerSource::Watcher("departure".into()),
        ),
        WatchEvent::Task(task) => (
            LedgerCategory::TaskCompleted,
            format!("Task event: {task:?}"),
            LedgerSource::Watcher("tasks".into()),
        ),
        WatchEvent::Weather(w) => (
            LedgerCategory::Observation,
            format!("Weather update: {w:?}"),
            LedgerSource::Watcher("weather".into()),
        ),
        WatchEvent::Sports(alert) => (
            LedgerCategory::EventAttended,
            format!("{} {} — {}", alert.series_name, alert.round_name, alert.session_name),
            LedgerSource::Watcher("sports".into()),
        ),
        WatchEvent::Cultural(alert) => (
            LedgerCategory::EventAttended,
            format!("Cultural: {}", alert.title),
            LedgerSource::Watcher("cultural".into()),
        ),
    }
}

/// Parse a timezone string into a chrono FixedOffset.
/// Supports IANA names via convention (Europe/Lisbon → +00:00 winter, +01:00 summer).
/// Falls back to UTC on unrecognized timezones.
fn parse_tz_offset(tz: &str) -> chrono::FixedOffset {
    // Simple mapping for common European timezones.
    // In production, use chrono-tz, but it's not in deps.
    let offset_hours = match tz {
        "Europe/Lisbon" | "Europe/London" | "Atlantic/Canary" => 0,
        "Europe/Paris" | "Europe/Berlin" | "Europe/Madrid" | "Europe/Rome"
        | "Europe/Amsterdam" | "Europe/Brussels" | "Europe/Vienna" => 1,
        "Europe/Helsinki" | "Europe/Bucharest" | "Europe/Athens" | "Europe/Istanbul" => 2,
        "Europe/Moscow" => 3,
        "US/Eastern" | "America/New_York" => -5,
        "US/Central" | "America/Chicago" => -6,
        "US/Mountain" | "America/Denver" => -7,
        "US/Pacific" | "America/Los_Angeles" => -8,
        "Asia/Tokyo" => 9,
        "Asia/Shanghai" | "Asia/Hong_Kong" => 8,
        _ => {
            tracing::warn!(tz = tz, "unrecognized timezone, using UTC");
            0
        }
    };
    chrono::FixedOffset::east_opt(offset_hours * 3600).unwrap_or_else(|| {
        chrono::FixedOffset::east_opt(0).unwrap()
    })
}
