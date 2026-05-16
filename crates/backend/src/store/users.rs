use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::DbPool;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalUser {
    pub id: String,
    pub email: String,
    pub cloud_token: Option<String>,
    pub license_tier: String,
    pub created_at: i64,
}

pub fn update_cloud_auth(pool: &DbPool, user_id: &str, token: &str, tier: &str, email: Option<&str>) {
    if let Ok(conn) = pool.get() {
        if let Some(email) = email {
            let _ = conn.execute(
                "UPDATE local_user SET cloud_token = ?1, license_tier = ?2, email = ?3 WHERE id = ?4",
                params![token, tier, email, user_id],
            );
        } else {
            let _ = conn.execute(
                "UPDATE local_user SET cloud_token = ?1, license_tier = ?2 WHERE id = ?3",
                params![token, tier, user_id],
            );
        }
    }
}

pub fn clear_cloud_token(pool: &DbPool, user_id: &str) {
    if let Ok(conn) = pool.get() {
        let _ = conn.execute(
            "UPDATE local_user SET cloud_token = NULL, license_tier = 'free' WHERE id = ?1",
            params![user_id],
        );
    }
}

pub fn get_user(pool: &DbPool, user_id: &str) -> Option<LocalUser> {
    let conn = pool.get().ok()?;
    conn.query_row(
        "SELECT id, email, cloud_token, license_tier, created_at
         FROM local_user WHERE id = ?1",
        params![user_id],
        |row| {
            Ok(LocalUser {
                id: row.get(0)?,
                email: row.get(1)?,
                cloud_token: row.get(2)?,
                license_tier: row.get(3)?,
                created_at: row.get(4)?,
            })
        },
    )
    .ok()
}
