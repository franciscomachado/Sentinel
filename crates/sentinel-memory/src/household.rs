/// Household shared surface — CRUD for family_events, meal_plan, shopping_list,
/// and household_tasks in the shared database.
///
/// Each instance connects to the household's shared.db (read-write for the
/// owning user, read-only via systemd filesystem isolation for others).

use chrono::{Datelike, Utc};
use sentinel_core::types::{HouseholdTask, MealEntry, ShoppingItem};
use sqlx::SqlitePool;

/// Handle to the household shared database.
#[derive(Clone)]
pub struct HouseholdStore {
    pool: SqlitePool,
    user_id: String,
}

impl HouseholdStore {
    pub fn new(pool: SqlitePool, user_id: String) -> Self {
        Self { pool, user_id }
    }

    // ── Meal Plan ──────────────────────────────────────────────────

    /// Add a meal to the plan.
    pub async fn add_meal(&self, entry: &MealEntry) -> anyhow::Result<i64> {
        let ingredients_json = serde_json::to_string(&entry.ingredients)?;

        let id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO meal_plan (date, meal_type, description, ingredients, created_by)
             VALUES (?, ?, ?, ?, ?)
             RETURNING id"
        )
        .bind(&entry.date)
        .bind(&entry.meal_type)
        .bind(&entry.description)
        .bind(&ingredients_json)
        .bind(&self.user_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(id)
    }

    /// Get meal plan for a date range.
    pub async fn meals_between(&self, from: &str, to: &str) -> anyhow::Result<Vec<MealEntry>> {
        let rows = sqlx::query_as::<_, MealRow>(
            "SELECT date, meal_type, description, ingredients, created_by
             FROM meal_plan WHERE date >= ? AND date <= ?
             ORDER BY date, meal_type"
        )
        .bind(from)
        .bind(to)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|r| r.into()).collect())
    }

    /// Get today's meal plan.
    pub async fn todays_meals(&self) -> anyhow::Result<Vec<MealEntry>> {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        self.meals_between(&today, &today).await
    }

    /// Get this week's meals (Mon–Sun).
    pub async fn this_weeks_meals(&self) -> anyhow::Result<Vec<MealEntry>> {
        let today = Utc::now().date_naive();
        let weekday = today.weekday().num_days_from_monday();
        let monday = today - chrono::Duration::days(weekday as i64);
        let sunday = monday + chrono::Duration::days(6);
        self.meals_between(
            &monday.format("%Y-%m-%d").to_string(),
            &sunday.format("%Y-%m-%d").to_string(),
        ).await
    }

    /// Remove a meal entry by id.
    pub async fn remove_meal(&self, id: i64) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM meal_plan WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ── Shopping List ──────────────────────────────────────────────

    /// Add an item to the shopping list.
    pub async fn add_shopping_item(&self, item: &str, category: Option<&str>, context: Option<&str>) -> anyhow::Result<i64> {
        let id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO shopping_list (item, category, added_by, context)
             VALUES (?, ?, ?, ?)
             RETURNING id"
        )
        .bind(item)
        .bind(category)
        .bind(&self.user_id)
        .bind(context)
        .fetch_one(&self.pool)
        .await?;

        Ok(id)
    }

    /// Get all unpurchased items on the shopping list.
    pub async fn shopping_list(&self) -> anyhow::Result<Vec<ShoppingItem>> {
        let rows = sqlx::query_as::<_, ShoppingRow>(
            "SELECT id, item, category, added_by, context, purchased
             FROM shopping_list WHERE purchased = 0
             ORDER BY added_at"
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|r| r.into()).collect())
    }

    /// Mark an item as purchased.
    pub async fn mark_purchased(&self, id: i64) -> anyhow::Result<bool> {
        let result = sqlx::query("UPDATE shopping_list SET purchased = 1 WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Remove a shopping item. Returns the item details (for partner notification).
    pub async fn remove_shopping_item(&self, id: i64) -> anyhow::Result<Option<ShoppingItem>> {
        let row = sqlx::query_as::<_, ShoppingRow>(
            "SELECT id, item, category, added_by, context, purchased
             FROM shopping_list WHERE id = ?"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(ref _r) = row {
            sqlx::query("DELETE FROM shopping_list WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
                .await?;
        }

        Ok(row.map(|r| r.into()))
    }

    /// Check if an item was added by a different household member.
    pub async fn item_added_by_other(&self, id: i64) -> anyhow::Result<Option<String>> {
        let added_by: Option<String> = sqlx::query_scalar(
            "SELECT added_by FROM shopping_list WHERE id = ?"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        match added_by {
            Some(by) if by != self.user_id => Ok(Some(by)),
            _ => Ok(None),
        }
    }

    // ── Family Events ──────────────────────────────────────────────

    /// Add a family event.
    pub async fn add_family_event(
        &self,
        id: &str,
        title: &str,
        start_time: &str,
        end_time: Option<&str>,
        location: Option<&str>,
        notes: Option<&str>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO family_events (id, title, start_time, end_time, location, notes, created_by)
             VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(id)
        .bind(title)
        .bind(start_time)
        .bind(end_time)
        .bind(location)
        .bind(notes)
        .bind(&self.user_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get upcoming family events (next N days).
    pub async fn upcoming_family_events(&self, days: u32) -> anyhow::Result<Vec<FamilyEvent>> {
        let now = Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string();
        let cutoff = (Utc::now() + chrono::Duration::days(days as i64))
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string();

        let rows = sqlx::query_as::<_, FamilyEventRow>(
            "SELECT id, title, start_time, end_time, location, notes, created_by
             FROM family_events WHERE start_time >= ? AND start_time <= ?
             ORDER BY start_time"
        )
        .bind(&now)
        .bind(&cutoff)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|r| r.into()).collect())
    }

    /// Delete a family event.
    pub async fn delete_family_event(&self, id: &str) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM family_events WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ── Household Tasks ────────────────────────────────────────────

    /// Add a household task (chore).
    pub async fn add_household_task(&self, task: &HouseholdTask) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO household_tasks (id, title, assigned_to, schedule_type, schedule_data, next_trigger)
             VALUES (?, ?, ?, ?, ?, ?)"
        )
        .bind(&task.id)
        .bind(&task.title)
        .bind(&task.assigned_to)
        .bind(&task.schedule_type)
        .bind(&task.schedule_data)
        .bind(&task.next_trigger)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get all household tasks.
    pub async fn household_tasks(&self) -> anyhow::Result<Vec<HouseholdTask>> {
        let rows = sqlx::query_as::<_, HouseholdTaskRow>(
            "SELECT id, title, assigned_to, schedule_type, schedule_data, next_trigger
             FROM household_tasks ORDER BY next_trigger"
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|r| r.into()).collect())
    }

    // ── Formatting ─────────────────────────────────────────────────

    /// Sync ingredients from this week's meal plan to the shopping list.
    /// Returns the number of items added (skips duplicates already on the list).
    pub async fn sync_meal_ingredients_to_shopping(&self) -> anyhow::Result<usize> {
        let meals = self.this_weeks_meals().await?;
        let current_items = self.shopping_list().await?;
        let existing: std::collections::HashSet<String> = current_items
            .iter()
            .map(|i| i.item.to_lowercase())
            .collect();

        let mut added = 0;
        for meal in &meals {
            for ingredient in &meal.ingredients {
                if !existing.contains(&ingredient.to_lowercase()) {
                    let context = format!("for {} ({})", meal.description, meal.meal_type);
                    self.add_shopping_item(ingredient, Some("Meal plan"), Some(&context)).await?;
                    added += 1;
                }
            }
        }
        Ok(added)
    }

    /// Format today's meal plan for the state compiler.
    pub async fn format_todays_meals(&self) -> String {
        match self.todays_meals().await {
            Ok(meals) if !meals.is_empty() => {
                let lines: Vec<String> = meals.iter().map(|m| {
                    let ingredients = if m.ingredients.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", m.ingredients.join(", "))
                    };
                    format!("- {} {}{ingredients}", m.meal_type, m.description)
                }).collect();
                lines.join("\n")
            }
            _ => String::new(),
        }
    }

    /// Format the shopping list for the state compiler.
    pub async fn format_shopping_list(&self) -> String {
        match self.shopping_list().await {
            Ok(items) if !items.is_empty() => {
                let lines: Vec<String> = items.iter().map(|i| {
                    let ctx = i.context.as_deref()
                        .map(|c| format!(" ({c})"))
                        .unwrap_or_default();
                    let by = if i.added_by != self.user_id {
                        format!(" [added by {}]", i.added_by)
                    } else {
                        String::new()
                    };
                    format!("- {}{ctx}{by}", i.item)
                }).collect();
                lines.join("\n")
            }
            _ => String::new(),
        }
    }

    /// Format upcoming family events for the state compiler.
    pub async fn format_family_events(&self, days: u32) -> String {
        match self.upcoming_family_events(days).await {
            Ok(events) if !events.is_empty() => {
                let lines: Vec<String> = events.iter().map(|e| {
                    let loc = e.location.as_deref()
                        .map(|l| format!(" at {l}"))
                        .unwrap_or_default();
                    format!("- {} {}{loc}", e.start_time, e.title)
                }).collect();
                lines.join("\n")
            }
            _ => String::new(),
        }
    }
}

// ── Row types for sqlx ─────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct MealRow {
    date: String,
    meal_type: String,
    description: String,
    ingredients: String,
    created_by: String,
}

impl From<MealRow> for MealEntry {
    fn from(r: MealRow) -> Self {
        let ingredients: Vec<String> = serde_json::from_str(&r.ingredients)
            .unwrap_or_default();
        MealEntry {
            date: r.date,
            meal_type: r.meal_type,
            description: r.description,
            ingredients,
            created_by: r.created_by,
        }
    }
}

#[derive(sqlx::FromRow)]
struct ShoppingRow {
    id: i64,
    item: String,
    category: Option<String>,
    added_by: String,
    context: Option<String>,
    purchased: bool,
}

impl From<ShoppingRow> for ShoppingItem {
    fn from(r: ShoppingRow) -> Self {
        ShoppingItem {
            id: Some(r.id),
            item: r.item,
            category: r.category,
            added_by: r.added_by,
            context: r.context,
            purchased: r.purchased,
        }
    }
}

/// A family event from the shared calendar.
#[derive(Debug, Clone)]
pub struct FamilyEvent {
    pub id: String,
    pub title: String,
    pub start_time: String,
    pub end_time: Option<String>,
    pub location: Option<String>,
    pub notes: Option<String>,
    pub created_by: String,
}

#[derive(sqlx::FromRow)]
struct FamilyEventRow {
    id: String,
    title: String,
    start_time: String,
    end_time: Option<String>,
    location: Option<String>,
    notes: Option<String>,
    created_by: String,
}

impl From<FamilyEventRow> for FamilyEvent {
    fn from(r: FamilyEventRow) -> Self {
        FamilyEvent {
            id: r.id,
            title: r.title,
            start_time: r.start_time,
            end_time: r.end_time,
            location: r.location,
            notes: r.notes,
            created_by: r.created_by,
        }
    }
}

#[derive(sqlx::FromRow)]
struct HouseholdTaskRow {
    id: String,
    title: String,
    assigned_to: Option<String>,
    schedule_type: String,
    schedule_data: String,
    next_trigger: Option<String>,
}

impl From<HouseholdTaskRow> for HouseholdTask {
    fn from(r: HouseholdTaskRow) -> Self {
        HouseholdTask {
            id: r.id,
            title: r.title,
            assigned_to: r.assigned_to,
            schedule_type: r.schedule_type,
            schedule_data: r.schedule_data,
            next_trigger: r.next_trigger,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_db() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = crate::db::open(&db_path).await.unwrap();
        (pool, dir)
    }

    #[tokio::test]
    async fn meal_plan_crud() {
        let (pool, _dir) = test_db().await;
        let store = HouseholdStore::new(pool, "john".into());

        let entry = MealEntry {
            date: "2026-02-21".into(),
            meal_type: "dinner".into(),
            description: "Spaghetti Bolognese".into(),
            ingredients: vec!["minced meat".into(), "pasta".into(), "tomato sauce".into()],
            created_by: "john".into(),
        };

        let id = store.add_meal(&entry).await.unwrap();
        assert!(id > 0);

        let meals = store.meals_between("2026-02-21", "2026-02-21").await.unwrap();
        assert_eq!(meals.len(), 1);
        assert_eq!(meals[0].description, "Spaghetti Bolognese");
        assert_eq!(meals[0].ingredients.len(), 3);
        assert_eq!(meals[0].created_by, "john");

        assert!(store.remove_meal(id).await.unwrap());
        let meals = store.meals_between("2026-02-21", "2026-02-21").await.unwrap();
        assert!(meals.is_empty());
    }

    #[tokio::test]
    async fn shopping_list_crud() {
        let (pool, _dir) = test_db().await;
        let store = HouseholdStore::new(pool, "john".into());

        let id = store.add_shopping_item("Milk", Some("Dairy"), Some("for cereal")).await.unwrap();
        assert!(id > 0);

        let items = store.shopping_list().await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].item, "Milk");
        assert_eq!(items[0].added_by, "john");

        // Mark purchased
        assert!(store.mark_purchased(id).await.unwrap());
        let items = store.shopping_list().await.unwrap();
        assert!(items.is_empty()); // purchased items filtered out

        // Add another and remove
        let id2 = store.add_shopping_item("Bread", None, None).await.unwrap();
        let removed = store.remove_shopping_item(id2).await.unwrap();
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().item, "Bread");
    }

    #[tokio::test]
    async fn shopping_item_added_by_other() {
        let (pool, _dir) = test_db().await;

        // Mary adds an item
        let sara_store = HouseholdStore::new(pool.clone(), "mary".into());
        let id = sara_store.add_shopping_item("Tofu", None, None).await.unwrap();

        // John checks who added it
        let john_store = HouseholdStore::new(pool, "john".into());
        let other = john_store.item_added_by_other(id).await.unwrap();
        assert_eq!(other, Some("mary".into()));

        // Mary checks — it's her own
        let own = sara_store.item_added_by_other(id).await.unwrap();
        assert_eq!(own, None);
    }

    #[tokio::test]
    async fn family_events_crud() {
        let (pool, _dir) = test_db().await;
        let store = HouseholdStore::new(pool, "john".into());

        // Add a future event
        let future = (Utc::now() + chrono::Duration::hours(2))
            .format("%Y-%m-%dT%H:%M:%S").to_string();
        store.add_family_event(
            "ev1", "Kids football", &future, None, Some("Campo da Constituição"), None,
        ).await.unwrap();

        let events = store.upcoming_family_events(7).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title, "Kids football");
        assert_eq!(events[0].location.as_deref(), Some("Campo da Constituição"));

        assert!(store.delete_family_event("ev1").await.unwrap());
        let events = store.upcoming_family_events(7).await.unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn household_tasks_crud() {
        let (pool, _dir) = test_db().await;
        let store = HouseholdStore::new(pool, "john".into());

        let task = HouseholdTask {
            id: "chore-1".into(),
            title: "Take out bins".into(),
            assigned_to: Some("john".into()),
            schedule_type: "weekly".into(),
            schedule_data: "tuesday".into(),
            next_trigger: Some("2026-02-24".into()),
        };

        store.add_household_task(&task).await.unwrap();
        let tasks = store.household_tasks().await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "Take out bins");
        assert_eq!(tasks[0].assigned_to.as_deref(), Some("john"));
    }

    #[tokio::test]
    async fn format_shopping_list_output() {
        let (pool, _dir) = test_db().await;

        // Mary adds items
        let sara_store = HouseholdStore::new(pool.clone(), "mary".into());
        sara_store.add_shopping_item("Tofu", None, Some("for stir fry")).await.unwrap();

        // John adds items
        let john_store = HouseholdStore::new(pool, "john".into());
        john_store.add_shopping_item("Beer", Some("Drinks"), None).await.unwrap();

        // John's view shows Mary's item flagged
        let formatted = john_store.format_shopping_list().await;
        assert!(formatted.contains("Tofu (for stir fry) [added by mary]"));
        assert!(formatted.contains("Beer")); // his own, no flag
        assert!(!formatted.contains("[added by john]"));
    }

    #[tokio::test]
    async fn cross_user_removal_notification() {
        let (pool, _dir) = test_db().await;

        // Mary adds 'Tofu'
        let mary = HouseholdStore::new(pool.clone(), "mary".into());
        let id = mary.add_shopping_item("Tofu", None, None).await.unwrap();

        // John's store — he wants to remove it
        let john = HouseholdStore::new(pool, "john".into());

        // Before removing, check if it was added by someone else
        let other = john.item_added_by_other(id).await.unwrap();
        assert_eq!(other, Some("mary".into())); // Yes — notify Mary

        // Remove it
        let removed = john.remove_shopping_item(id).await.unwrap();
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().item, "Tofu");

        // List is now empty
        let items = john.shopping_list().await.unwrap();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn meal_ingredient_sync_to_shopping() {
        let (pool, _dir) = test_db().await;
        let store = HouseholdStore::new(pool, "john".into());

        // Add a meal for today
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let meal = MealEntry {
            date: today,
            meal_type: "dinner".into(),
            description: "Pad Thai".into(),
            ingredients: vec!["rice noodles".into(), "tofu".into(), "peanuts".into()],
            created_by: "john".into(),
        };
        store.add_meal(&meal).await.unwrap();

        // Already have peanuts on the list
        store.add_shopping_item("Peanuts", None, None).await.unwrap();

        // Sync — should add 2 (rice noodles + tofu), skip peanuts
        let added = store.sync_meal_ingredients_to_shopping().await.unwrap();
        assert_eq!(added, 2);

        let items = store.shopping_list().await.unwrap();
        assert_eq!(items.len(), 3); // peanuts + rice noodles + tofu

        // Syncing again should add 0 (no new ingredients)
        let added2 = store.sync_meal_ingredients_to_shopping().await.unwrap();
        assert_eq!(added2, 0);
    }
}
