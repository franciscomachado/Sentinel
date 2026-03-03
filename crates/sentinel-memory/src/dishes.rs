use sentinel_core::types::Dish;
use sqlx::SqlitePool;

/// Persistent store for the user's personal dish/recipe catalog.
///
/// Backed by the `dishes` table (migration 008) in the main personal database.
/// Works without household mode — household mode additionally mirrors dishes
/// into the shared household pool via `HouseholdStore::add_dish`.
#[derive(Clone)]
pub struct DishStore {
    pool: SqlitePool,
}

impl DishStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Add a dish to the personal catalog.
    ///
    /// Returns `(id, is_new)`. If a dish with the same name already exists
    /// (case-insensitive) the existing row id is returned and `is_new` is `false`.
    pub async fn add(&self, dish: &Dish) -> anyhow::Result<(i64, bool)> {
        // INSERT OR IGNORE — silently skips if lower(name) already exists.
        let result = sqlx::query(
            "INSERT OR IGNORE INTO dishes (name, protein, carb, notes) VALUES (?, ?, ?, ?)",
        )
        .bind(&dish.name)
        .bind(dish.protein.as_deref())
        .bind(dish.carb.as_deref())
        .bind(dish.notes.as_deref())
        .execute(&self.pool)
        .await?;

        let is_new = result.rows_affected() == 1;

        // Fetch the canonical row id (whether just inserted or pre-existing).
        let id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM dishes WHERE lower(name) = lower(?)",
        )
        .bind(&dish.name)
        .fetch_one(&self.pool)
        .await?;

        Ok((id, is_new))
    }

    /// Return all dishes ordered by name.
    pub async fn list(&self) -> anyhow::Result<Vec<Dish>> {
        #[allow(dead_code)]
        #[derive(sqlx::FromRow)]
        struct Row {
            id: i64,
            name: String,
            protein: Option<String>,
            carb: Option<String>,
            notes: Option<String>,
        }
        let rows = sqlx::query_as::<_, Row>(
            "SELECT id, name, protein, carb, notes FROM dishes ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| Dish {
                id: Some(r.id),
                name: r.name,
                protein: r.protein,
                carb: r.carb,
                notes: r.notes,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[tokio::test]
    async fn add_and_list_dish() {
        let pool = crate::db::open(Path::new(":memory:")).await.unwrap();
        let store = DishStore::new(pool);

        let dish = Dish {
            id: None,
            name: "Pescada cozida com batatas".to_string(),
            protein: Some("pescada".to_string()),
            carb: Some("batatas".to_string()),
            notes: None,
        };

        let (id, is_new) = store.add(&dish).await.unwrap();
        assert!(id > 0);
        assert!(is_new);

        // Duplicate insert — idempotent, same id returned, is_new = false.
        let (id2, is_new2) = store.add(&dish).await.unwrap();
        assert_eq!(id, id2);
        assert!(!is_new2);

        let all = store.list().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Pescada cozida com batatas");
    }
}
