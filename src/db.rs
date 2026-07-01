//! Database connection, schema, and query helpers.

use crate::models::{Expense, Group, Member, Settlement};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS groups (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    currency    TEXT NOT NULL DEFAULT 'SEK',
    recovery    TEXT,
    closed_at   TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    last_active TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS members (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    group_id    TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    token_hash  TEXT NOT NULL,
    is_owner    INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS expenses (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    group_id    TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    payer_id    INTEGER NOT NULL REFERENCES members(id),
    amount      INTEGER NOT NULL,
    description TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    deleted_at  TEXT
);

CREATE TABLE IF NOT EXISTS expense_shares (
    expense_id  INTEGER NOT NULL REFERENCES expenses(id) ON DELETE CASCADE,
    member_id   INTEGER NOT NULL REFERENCES members(id),
    amount      INTEGER NOT NULL,
    PRIMARY KEY (expense_id, member_id)
);

CREATE TABLE IF NOT EXISTS settlements (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    group_id    TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    from_id     INTEGER NOT NULL REFERENCES members(id),
    to_id       INTEGER NOT NULL REFERENCES members(id),
    amount      INTEGER NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    deleted_at  TEXT
);

CREATE INDEX IF NOT EXISTS idx_members_group   ON members(group_id);
CREATE INDEX IF NOT EXISTS idx_expenses_group  ON expenses(group_id);
CREATE INDEX IF NOT EXISTS idx_settle_group    ON settlements(group_id);
"#;

/// Connect (creating the file if needed), enable foreign keys, and apply the schema.
pub async fn connect(path: &str) -> Result<SqlitePool, sqlx::Error> {
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await?;
    sqlx::raw_sql(SCHEMA).execute(&pool).await?;
    Ok(pool)
}

/// Bump a group's last-active timestamp so it survives auto-expiry.
pub async fn touch_group(pool: &SqlitePool, group_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE groups SET last_active = datetime('now') WHERE id = ?")
        .bind(group_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn load_group(pool: &SqlitePool, id: &str) -> Result<Option<Group>, sqlx::Error> {
    sqlx::query_as::<_, Group>("SELECT * FROM groups WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn list_members(pool: &SqlitePool, group_id: &str) -> Result<Vec<Member>, sqlx::Error> {
    sqlx::query_as::<_, Member>("SELECT * FROM members WHERE group_id = ? ORDER BY id")
        .bind(group_id)
        .fetch_all(pool)
        .await
}

/// Resolve the member for a group given the raw device token from their cookie.
pub async fn member_by_token(
    pool: &SqlitePool,
    group_id: &str,
    raw_token: &str,
) -> Result<Option<Member>, sqlx::Error> {
    let hash = crate::ids::hash_token(raw_token);
    sqlx::query_as::<_, Member>("SELECT * FROM members WHERE group_id = ? AND token_hash = ?")
        .bind(group_id)
        .bind(hash)
        .fetch_optional(pool)
        .await
}

/// Non-deleted expenses for a group, newest first.
pub async fn list_expenses(pool: &SqlitePool, group_id: &str) -> Result<Vec<Expense>, sqlx::Error> {
    sqlx::query_as::<_, Expense>(
        "SELECT id, group_id, payer_id, amount, description, created_at
         FROM expenses WHERE group_id = ? AND deleted_at IS NULL
         ORDER BY id DESC",
    )
    .bind(group_id)
    .fetch_all(pool)
    .await
}

/// Non-deleted settlements for a group, newest first.
pub async fn list_settlements(
    pool: &SqlitePool,
    group_id: &str,
) -> Result<Vec<Settlement>, sqlx::Error> {
    sqlx::query_as::<_, Settlement>(
        "SELECT id, group_id, from_id, to_id, amount, created_at
         FROM settlements WHERE group_id = ? AND deleted_at IS NULL
         ORDER BY id DESC",
    )
    .bind(group_id)
    .fetch_all(pool)
    .await
}

/// `(payer_id, amount)` pairs for balance math.
pub async fn expense_payments(
    pool: &SqlitePool,
    group_id: &str,
) -> Result<Vec<(i64, i64)>, sqlx::Error> {
    sqlx::query_as::<_, (i64, i64)>(
        "SELECT payer_id, amount FROM expenses
         WHERE group_id = ? AND deleted_at IS NULL",
    )
    .bind(group_id)
    .fetch_all(pool)
    .await
}

/// `(member_id, amount)` share pairs across all non-deleted expenses in the group.
pub async fn expense_share_rows(
    pool: &SqlitePool,
    group_id: &str,
) -> Result<Vec<(i64, i64)>, sqlx::Error> {
    sqlx::query_as::<_, (i64, i64)>(
        "SELECT s.member_id, s.amount
         FROM expense_shares s
         JOIN expenses e ON e.id = s.expense_id
         WHERE e.group_id = ? AND e.deleted_at IS NULL",
    )
    .bind(group_id)
    .fetch_all(pool)
    .await
}

/// `(from_id, to_id, amount)` triples for balance math.
pub async fn settlement_rows(
    pool: &SqlitePool,
    group_id: &str,
) -> Result<Vec<(i64, i64, i64)>, sqlx::Error> {
    sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT from_id, to_id, amount FROM settlements
         WHERE group_id = ? AND deleted_at IS NULL",
    )
    .bind(group_id)
    .fetch_all(pool)
    .await
}

/// The member ids included in a given expense (for display).
pub async fn expense_participants(
    pool: &SqlitePool,
    expense_id: i64,
) -> Result<Vec<i64>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (i64,)>(
        "SELECT member_id FROM expense_shares WHERE expense_id = ? ORDER BY member_id",
    )
    .bind(expense_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Delete groups that were never claimed (no recovery set) and have been inactive
/// past the cutoff. Child rows go with them via ON DELETE CASCADE. Returns the count.
pub async fn expire_stale_groups(pool: &SqlitePool, inactive_days: i64) -> Result<u64, sqlx::Error> {
    let cutoff = format!("-{inactive_days} days");
    let res = sqlx::query(
        "DELETE FROM groups
         WHERE recovery IS NULL
           AND last_active < datetime('now', ?)",
    )
    .bind(cutoff)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}
