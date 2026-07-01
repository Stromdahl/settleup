//! Row structs mapped from the database. Timestamps are kept as the raw SQLite text
//! (`YYYY-MM-DD HH:MM:SS`, UTC) — we only ever display or compare them in SQL, so
//! there's no need to pull in a datetime crate.
//!
//! These structs mirror the table columns for faithful `SELECT *` mapping; not every
//! column is read back out in every code path.
#![allow(dead_code)]

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub currency: String,
    /// Optional recovery passphrase hash. Its presence marks the group as "claimed"
    /// (kept forever, exempt from auto-expiry).
    pub recovery: Option<String>,
    pub closed_at: Option<String>,
    pub created_at: String,
    pub last_active: String,
}

impl Group {
    pub fn is_closed(&self) -> bool {
        self.closed_at.is_some()
    }
    pub fn has_recovery(&self) -> bool {
        self.recovery.is_some()
    }
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Member {
    pub id: i64,
    pub group_id: String,
    pub name: String,
    pub token_hash: String,
    pub is_owner: bool,
    pub created_at: String,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Expense {
    pub id: i64,
    pub group_id: String,
    pub payer_id: i64,
    pub amount: i64,
    pub description: String,
    pub created_at: String,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Settlement {
    pub id: i64,
    pub group_id: String,
    pub from_id: i64,
    pub to_id: i64,
    pub amount: i64,
    pub created_at: String,
}
