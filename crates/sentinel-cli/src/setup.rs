/// Interactive onboarding wizard — runs as a multi-turn conversation via Signal.
///
/// Uses Claude to have a natural conversation with the user, building an
/// initial profile (household, food prefs, schedule, interests, notification
/// preferences). The result is persisted as memories so Sentinel can produce
/// a useful morning briefing on day one.

use anyhow::{Context, Result};
use sentinel_core::config::SentinelConfig;
use sentinel_core::types::Dish;
use sentinel_cortex::prompt::{LlmRequest, Message};
use sentinel_cortex::provider::AiProvider;
use sentinel_gate::signal::SignalClient;
use sentinel_memory::household::HouseholdStore;
use sentinel_memory::state::StateManager;
use sqlx::SqlitePool;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

/// Onboarding stages, tracked so the conversation can resume after restart.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Stage {
    Welcome,
    Household,
    FoodPreferences,
    DishCatalog,
    Schedule,
    Interests,
    Notifications,
    Complete,
}

impl Stage {
    fn next(&self) -> Stage {
        match self {
            Stage::Welcome => Stage::Household,
            Stage::Household => Stage::FoodPreferences,
            Stage::FoodPreferences => Stage::DishCatalog,
            Stage::DishCatalog => Stage::Schedule,
            Stage::Schedule => Stage::Interests,
            Stage::Interests => Stage::Notifications,
            Stage::Notifications => Stage::Complete,
            Stage::Complete => Stage::Complete,
        }
    }
}

/// The system prompt for the onboarding meta-conversation.
const ONBOARDING_SYSTEM: &str = r#"You are {name}, a personal AI assistant having an onboarding conversation via Signal.

Your job is to learn about the user's life in a friendly, brief, conversational way. You are NOT a form — this is a natural chat.

Rules:
- Keep messages short (2-4 sentences max)
- Ask one topic at a time
- Acknowledge what they said before moving on
- Use a warm but concise tone — like a thoughtful friend
- Never be sycophantic or overly enthusiastic
- If they give a short answer, that's fine — don't push for detail they haven't offered

After each user reply, respond with JSON at the end of your message (on a new line, wrapped in triple backticks with json):
```json
{"memories": ["fact 1", "fact 2"], "tags": ["topic"], "done_with_stage": true/false, "dishes": []}
```

"memories" = facts to remember (empty array if nothing new learned)
"tags" = topic tags for the memories (e.g. "household", "food", "schedule")
"done_with_stage" = true when you've covered enough for the current topic
"dishes" = during the dish_catalog stage only: structured list of dishes mentioned this turn, e.g.
  [{"name": "Arroz de polvo", "protein": "polvo", "carb": "arroz"}, {"name": "Frango assado", "protein": "frango", "carb": "batata"}]
  Leave as [] for all other stages.

Current stage: {stage}
Topics to cover per stage:
- household: who lives with them, ages, pets
- food: cooking frequency, regular dishes, dietary restrictions, partner preferences
- dish_catalog: list 5-15 dishes they cook regularly — for each dish include name, protein source, and carb; for this stage, populate the `dishes` JSON field (see below)
- schedule: work hours, commute, morning routine, evening routine
- interests: hobbies, sports, cultural interests, weekend activities
- notifications: when they don't want to be disturbed, how urgent is urgent

Previous context (what we already know):
{context}
"#;

pub struct SetupWizard {
    config: SentinelConfig,
    signal: SignalClient,
    client: Box<dyn AiProvider>,
    state: StateManager,
    household: HouseholdStore,
    user_number: String,
}

impl SetupWizard {
    pub fn new(
        config: &SentinelConfig,
        signal: SignalClient,
        client: Box<dyn AiProvider>,
        pool: SqlitePool,
    ) -> Result<Self> {
        let signal_config = config.signal.as_ref()
            .context("Signal must be configured for onboarding")?;

        let user_number = signal_config.allow_from.first()
            .context("At least one allowed Signal number is required")?
            .clone();

        let user_id = config.user.name.to_lowercase();
        Ok(Self {
            config: config.clone(),
            signal,
            client,
            state: StateManager::new(pool.clone()),
            household: HouseholdStore::new(pool, user_id),
            user_number,
        })
    }

    /// Run the onboarding conversation. Blocks until complete or user says "done".
    pub async fn run(&self) -> Result<()> {
        println!("Starting onboarding conversation via Signal...");
        let assistant = self.config.user.assistant_name();
        println!("{} will chat with {} to build an initial profile.", assistant, self.user_number);

        let mut stage = Stage::Welcome;
        let mut conversation: Vec<(String, String)> = Vec::new(); // (role, content)
        let mut known_facts: Vec<String> = Vec::new();

        // Send welcome
        let welcome = format!(
            "Hi, I'm {}. I'll be your personal assistant.\n\
             I need to learn a few things about you — about 10 minutes.\n\
             You can always update these later.\n\n\
             Let's start: who's in your household?",
            assistant
        );
        self.signal.send_message(&self.user_number, &welcome).await?;
        stage = stage.next(); // Move to Household

        println!("Welcome sent. Waiting for replies...");
        println!("(The conversation will happen in Signal. Watch this terminal for progress.)");

        // Poll for replies using signal-cli receive
        loop {
            if stage == Stage::Complete {
                break;
            }

            // Wait for user reply
            let reply = self.wait_for_reply().await?;

            // Check for early exit
            let lower = reply.to_lowercase();
            if lower == "done" || lower == "skip" || lower == "stop" {
                self.signal.send_message(
                    &self.user_number,
                    "Got it! You can always tell me more later — I'll keep learning as we go. 👋",
                ).await?;
                break;
            }

            conversation.push(("user".into(), reply.clone()));

            // Build context from known facts
            let context = if known_facts.is_empty() {
                "Nothing yet — this is the start of the conversation.".to_string()
            } else {
                known_facts.join("\n")
            };

            let system = ONBOARDING_SYSTEM
                .replace("{name}", assistant)
                .replace("{stage}", &format!("{:?}", stage).to_lowercase())
                .replace("{context}", &context);

            // Build messages for the AI provider
            let mut llm_messages = Vec::new();
            for (role, content) in &conversation {
                llm_messages.push(Message {
                    role: role.clone(),
                    content: content.clone(),
                });
            }

            let request = LlmRequest {
                model: self.client.model().to_owned(),
                max_tokens: 500,
                system,
                messages: llm_messages,
            };

            let response_text = self.client.chat(request).await?;
            let (visible_text, metadata) = parse_onboarding_response(&response_text);

            // Send the visible part to user
            if !visible_text.is_empty() {
                self.signal.send_message(&self.user_number, &visible_text).await?;
                conversation.push(("assistant".into(), response_text.clone()));
            }

            // Process metadata
            if let Some(meta) = metadata {
                let tags: Vec<String> = meta.tags.iter().map(|s| s.to_string()).collect();
                for memory in &meta.memories {
                    if !memory.is_empty() {
                        known_facts.push(memory.clone());
                        if let Err(e) = self.state.add_memory(memory, &tags, "onboarding").await {
                            tracing::warn!(error = %e, "failed to persist onboarding memory");
                        }
                    }
                }

                // Persist any dishes extracted during the dish_catalog stage
                if !meta.dishes.is_empty() {
                    for d in &meta.dishes {
                        let dish = Dish {
                            id: None,
                            name: d.name.clone(),
                            protein: d.protein.clone(),
                            carb: d.carb.clone(),
                            notes: d.notes.clone(),
                        };
                        match self.household.add_dish(&dish).await {
                            Ok(_) => known_facts.push(format!("Dish in catalog: {}", dish.name)),
                            Err(e) => tracing::warn!(error = %e, dish = %dish.name, "failed to save dish"),
                        }
                    }
                    println!("Saved {} dishes to catalog.", meta.dishes.len());
                }

                if meta.done_with_stage {
                    stage = stage.next();
                    println!("Stage complete, moving to: {:?}", stage);
                }
            }
        }

        // Send a farewell summary of what we learned
        if !known_facts.is_empty() {
            let summary_system = format!(
                "You are {assistant}, a personal AI assistant. The user just finished onboarding. \
                 Send a brief, warm wrap-up message (4-6 sentences max). \
                 Summarise what you learned about them in a natural way — \
                 don't list facts, weave them into a short paragraph. \
                 End by saying you're ready to help and they can always update things later."
            );
            let summary_request = LlmRequest {
                model: self.client.model().to_owned(),
                max_tokens: 400,
                system: summary_system,
                messages: vec![Message {
                    role: "user".into(),
                    content: format!("Here's what I learned:\n{}", known_facts.join("\n")),
                }],
            };
            match self.client.chat(summary_request).await {
                Ok(summary) => {
                    let _ = self.signal.send_message(&self.user_number, &summary).await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to generate onboarding summary");
                    let _ = self.signal.send_message(
                        &self.user_number,
                        &format!("All set! I've memorised {} things about you. \
                                  You can always update them later. 👋", known_facts.len()),
                    ).await;
                }
            }
        }

        // Persist a marker that onboarding was completed
        self.state.add_memory(
            "Onboarding conversation completed",
            &["system".into()],
            "onboarding",
        ).await?;

        println!("Onboarding complete! {} memories recorded.", known_facts.len());
        Ok(())
    }

    /// Wait for a Signal message from the user via signal-cli Unix socket.
    ///
    /// signal-cli in daemon mode already receives messages, so the HTTP
    /// `receive` RPC method is blocked. Instead we connect to the Unix
    /// socket and subscribe via `subscribeReceive`, which pushes incoming
    /// envelopes as JSON-RPC notifications.
    async fn wait_for_reply(&self) -> Result<String> {
        let signal_config = self.config.signal.as_ref()
            .context("Signal config required")?;
        let socket_path = signal_config.signal_socket();
        tracing::debug!(path = %socket_path, "connecting to signal-cli socket");

        let stream = tokio::net::UnixStream::connect(&socket_path)
            .await
            .with_context(|| format!(
                "failed to connect to signal-cli socket at {socket_path}. \
                 Make sure signal-cli daemon is running with --socket"
            ))?;

        let (reader, mut writer) = stream.into_split();

        // Subscribe to incoming messages
        let subscribe = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "subscribeReceive",
            "params": { "account": signal_config.account },
            "id": "sentinel-onboard"
        });
        let mut msg = serde_json::to_string(&subscribe)?;
        msg.push('\n');
        writer.write_all(msg.as_bytes()).await?;
        writer.flush().await?;

        let mut buf_reader = tokio::io::BufReader::new(reader);
        let mut line = String::new();

        // Read subscription acknowledgement
        buf_reader.read_line(&mut line).await?;
        tracing::debug!(response = %line.trim(), "subscription response");
        let ack: serde_json::Value = serde_json::from_str(line.trim())
            .context("invalid JSON in subscription response")?;
        if let Some(err) = ack.get("error") {
            anyhow::bail!("signal-cli subscription failed: {err}");
        }

        // Read incoming notifications
        loop {
            line.clear();
            let n = buf_reader.read_line(&mut line).await?;
            if n == 0 {
                anyhow::bail!("signal-cli socket closed unexpectedly");
            }

            let body: serde_json::Value = match serde_json::from_str(line.trim()) {
                Ok(v) => v,
                Err(e) => {
                    tracing::debug!(error = %e, "non-JSON line from socket, skipping");
                    continue;
                }
            };

            // JSON-RPC subscription notifications arrive as:
            // {"jsonrpc":"2.0","method":"receive","params":{"subscription":"…","result":{…}}}
            let envelope = match body.pointer("/params/result") {
                Some(r) => r,
                None => continue,
            };
            // signal-cli may wrap the payload in an "envelope" key
            let envelope = envelope.get("envelope").unwrap_or(envelope);

            let data = match envelope.get("dataMessage") {
                Some(d) => d,
                None => continue,
            };

            let message_text = match data.get("message").and_then(|m| m.as_str()) {
                Some(t) if !t.is_empty() => t,
                _ => continue,
            };

            let sender = envelope
                .get("sourceNumber")
                .or_else(|| envelope.get("source"))
                .and_then(|s| s.as_str())
                .unwrap_or("");

            if sender == self.user_number {
                tracing::debug!(sender, text = message_text, "received reply via socket");
                return Ok(message_text.to_string());
            }
        }
    }
}

/// Metadata extracted from Claude's onboarding response.
#[derive(Debug)]
struct OnboardingMeta {
    memories: Vec<String>,
    tags: Vec<String>,
    done_with_stage: bool,
    dishes: Vec<OnboardingDish>,
}

/// A dish extracted from the dish_catalog stage.
#[derive(Debug)]
struct OnboardingDish {
    name: String,
    protein: Option<String>,
    carb: Option<String>,
    notes: Option<String>,
}

/// Parse Claude's response into visible text and structured metadata.
fn parse_onboarding_response(response: &str) -> (String, Option<OnboardingMeta>) {
    // Look for JSON block at the end
    let json_marker = "```json";
    let json_end = "```";

    if let Some(json_start) = response.rfind(json_marker) {
        let visible = response[..json_start].trim().to_string();
        let json_block = &response[json_start + json_marker.len()..];
        if let Some(end) = json_block.find(json_end) {
            let json_str = json_block[..end].trim();
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_str) {
                let memories = parsed.get("memories")
                    .and_then(|m| m.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                let tags = parsed.get("tags")
                    .and_then(|t| t.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                let done_with_stage = parsed.get("done_with_stage")
                    .and_then(|d| d.as_bool())
                    .unwrap_or(false);
                let dishes = parsed.get("dishes")
                    .and_then(|d| d.as_array())
                    .map(|arr| {
                        arr.iter().filter_map(|v| {
                            let name = v.get("name").and_then(|n| n.as_str())?.to_string();
                            Some(OnboardingDish {
                                name,
                                protein: v.get("protein").and_then(|p| p.as_str()).map(String::from),
                                carb: v.get("carb").and_then(|c| c.as_str()).map(String::from),
                                notes: v.get("notes").and_then(|n| n.as_str()).map(String::from),
                            })
                        }).collect::<Vec<_>>()
                    })
                    .unwrap_or_default();

                return (visible, Some(OnboardingMeta { memories, tags, done_with_stage, dishes }));
            }
        }
    }

    (response.to_string(), None)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_response_with_metadata() {
        let response = "Family of four with two school-age kids, got it!\n\
                        Do you cook most nights, or mostly order in?\n\
                        ```json\n\
                        {\"memories\": [\"Household: user, wife Mary, 2 kids (8 and 11)\"], \"tags\": [\"household\"], \"done_with_stage\": false}\n\
                        ```";
        let (visible, meta) = parse_onboarding_response(response);
        assert!(visible.contains("Family of four"));
        assert!(!visible.contains("json"));
        let meta = meta.unwrap();
        assert_eq!(meta.memories.len(), 1);
        assert!(meta.memories[0].contains("Mary"));
        assert_eq!(meta.tags, vec!["household"]);
        assert!(!meta.done_with_stage);
    }

    #[test]
    fn parse_response_without_metadata() {
        let response = "Hello! Let's get started.";
        let (visible, meta) = parse_onboarding_response(response);
        assert_eq!(visible, "Hello! Let's get started.");
        assert!(meta.is_none());
    }

    #[test]
    fn parse_response_done_with_stage() {
        let response = "Great, I've got a good picture of your household.\nNow let's talk about food.\n\
                        ```json\n\
                        {\"memories\": [\"Has a cat named Luna\"], \"tags\": [\"household\"], \"done_with_stage\": true}\n\
                        ```";
        let (visible, meta) = parse_onboarding_response(response);
        assert!(visible.contains("household"));
        let meta = meta.unwrap();
        assert!(meta.done_with_stage);
    }

    #[test]
    fn stage_progression() {
        let mut stage = Stage::Welcome;
        stage = stage.next();
        assert_eq!(stage, Stage::Household);
        stage = stage.next();
        assert_eq!(stage, Stage::FoodPreferences);
        stage = stage.next();
        assert_eq!(stage, Stage::DishCatalog);
        stage = stage.next();
        assert_eq!(stage, Stage::Schedule);
        stage = stage.next();
        assert_eq!(stage, Stage::Interests);
        stage = stage.next();
        assert_eq!(stage, Stage::Notifications);
        stage = stage.next();
        assert_eq!(stage, Stage::Complete);
        stage = stage.next();
        assert_eq!(stage, Stage::Complete);
    }

    #[test]
    fn parse_response_with_dishes() {
        let response = "Great — got those two dishes!\n\
                        ```json\n\
                        {\"memories\": [], \"tags\": [\"food\"], \"done_with_stage\": false, \
                        \"dishes\": [{\"name\": \"Arroz de polvo\", \"protein\": \"polvo\", \"carb\": \"arroz\"}, \
                        {\"name\": \"Frango assado\", \"protein\": \"frango\", \"carb\": \"batata\"}]}\n\
                        ```";
        let (visible, meta) = parse_onboarding_response(response);
        assert!(visible.contains("Great"));
        let meta = meta.unwrap();
        assert_eq!(meta.dishes.len(), 2);
        assert_eq!(meta.dishes[0].name, "Arroz de polvo");
        assert_eq!(meta.dishes[0].protein.as_deref(), Some("polvo"));
        assert_eq!(meta.dishes[0].carb.as_deref(), Some("arroz"));
        assert_eq!(meta.dishes[1].name, "Frango assado");
    }
}
