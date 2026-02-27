use serde::{Deserialize, Serialize};

const API_BASE: &str = "https://api.getbring.com/rest/v2";

/// Bring! shopping list client.
#[derive(Clone)]
pub struct BringClient {
    http: reqwest::Client,
    auth: BringAuth,
}

#[derive(Clone)]
struct BringAuth {
    access_token: String,
    user_uuid: String,
    list_uuid: String,
}

#[derive(Deserialize)]
struct AuthResponse {
    #[serde(rename = "access_token")]
    access_token: String,
    #[serde(rename = "uuid")]
    uuid: String,
    #[serde(rename = "bringListUUID")]
    bring_list_uuid: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BringListItem {
    pub name: String,
    #[serde(default)]
    pub specification: String,
}

#[derive(Deserialize)]
struct BringListResponse {
    #[serde(default)]
    purchase: Vec<BringListItem>,
}

impl BringClient {
    /// Create a new Bring client by authenticating with email and password.
    pub async fn login(email: &str, password: &str) -> anyhow::Result<Self> {
        let http = reqwest::Client::new();

        let resp = http
            .post(format!("{API_BASE}/bringauth"))
            .form(&[("email", email), ("password", password)])
            .send()
            .await?;

        if !resp.status().is_success() {
            anyhow::bail!("Bring auth failed: {}", resp.status());
        }

        let auth_resp: AuthResponse = resp.json().await?;

        Ok(Self {
            http,
            auth: BringAuth {
                access_token: auth_resp.access_token,
                user_uuid: auth_resp.uuid,
                list_uuid: auth_resp.bring_list_uuid,
            },
        })
    }

    /// Add an item to the shopping list.
    pub async fn add_item(&self, name: &str, specification: &str) -> anyhow::Result<()> {
        let url = format!(
            "{API_BASE}/bringlists/{}/items",
            self.auth.list_uuid
        );

        let resp = self
            .http
            .put(&url)
            .header("Authorization", format!("Bearer {}", self.auth.access_token))
            .header("X-BRING-USER-UUID", &self.auth.user_uuid)
            .form(&[
                ("uuid", self.auth.list_uuid.as_str()),
                ("purchase", name),
                ("specification", specification),
            ])
            .send()
            .await?;

        if !resp.status().is_success() {
            anyhow::bail!("Bring add_item failed: {}", resp.status());
        }

        tracing::info!(item = name, "added to Bring list");
        Ok(())
    }

    /// Remove an item from the shopping list.
    pub async fn remove_item(&self, name: &str) -> anyhow::Result<()> {
        let url = format!(
            "{API_BASE}/bringlists/{}/items",
            self.auth.list_uuid
        );

        let resp = self
            .http
            .put(&url)
            .header("Authorization", format!("Bearer {}", self.auth.access_token))
            .header("X-BRING-USER-UUID", &self.auth.user_uuid)
            .form(&[
                ("uuid", self.auth.list_uuid.as_str()),
                ("remove", name),
            ])
            .send()
            .await?;

        if !resp.status().is_success() {
            anyhow::bail!("Bring remove_item failed: {}", resp.status());
        }

        tracing::info!(item = name, "removed from Bring list");
        Ok(())
    }

    /// Get all items currently on the shopping list.
    pub async fn get_list(&self) -> anyhow::Result<Vec<BringListItem>> {
        let url = format!(
            "{API_BASE}/bringlists/{}", self.auth.list_uuid
        );

        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.auth.access_token))
            .header("X-BRING-USER-UUID", &self.auth.user_uuid)
            .send()
            .await?;

        if !resp.status().is_success() {
            anyhow::bail!("Bring get_list failed: {}", resp.status());
        }

        let list: BringListResponse = resp.json().await?;
        Ok(list.purchase)
    }

    /// Format the current shopping list as context for the state compiler.
    pub async fn summary(&self) -> String {
        match self.get_list().await {
            Ok(items) if !items.is_empty() => {
                let lines: Vec<String> = items
                    .iter()
                    .map(|i| {
                        if i.specification.is_empty() {
                            format!("- {}", i.name)
                        } else {
                            format!("- {} ({})", i.name, i.specification)
                        }
                    })
                    .collect();
                lines.join("\n")
            }
            Ok(_) => String::new(),
            Err(e) => {
                tracing::warn!(error = %e, "failed to fetch Bring list for context");
                String::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_response_deserializes() {
        let json = r#"{
            "access_token": "tok_123",
            "uuid": "user-uuid",
            "bringListUUID": "list-uuid",
            "publicUuid": "pub-uuid"
        }"#;
        let resp: AuthResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.access_token, "tok_123");
        assert_eq!(resp.uuid, "user-uuid");
        assert_eq!(resp.bring_list_uuid, "list-uuid");
    }

    #[test]
    fn list_response_deserializes() {
        let json = r#"{
            "uuid": "list-uuid",
            "purchase": [
                {"name": "Milk", "specification": "Semi-skimmed"},
                {"name": "Bread", "specification": ""}
            ],
            "recently": []
        }"#;
        let resp: BringListResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.purchase.len(), 2);
        assert_eq!(resp.purchase[0].name, "Milk");
        assert_eq!(resp.purchase[0].specification, "Semi-skimmed");
    }
}
