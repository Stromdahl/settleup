//! HTTP route handlers.

use axum::extract::{Path, State};
use axum::http::header::HOST;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use maud::Markup;
use serde::Deserialize;
use sqlx::SqlitePool;
use std::collections::HashMap;

use crate::db;
use crate::ids;
use crate::money::parse_amount;
use crate::settle::{self, equal_shares};
use crate::views::{self, GroupView};

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    /// Public base URL (e.g. `https://settleup.example`). If unset, derived from the
    /// request Host header (fine for local use).
    pub base_url: Option<String>,
    /// Set the `Secure` flag on cookies (enable when served over HTTPS).
    pub secure_cookies: bool,
}

// --- Error handling -------------------------------------------------------------

pub enum AppError {
    NotFound,
    Forbidden,
    Db(sqlx::Error),
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        AppError::Db(e)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::NotFound => (StatusCode::NOT_FOUND, "Not found").into_response(),
            AppError::Forbidden => (StatusCode::FORBIDDEN, "Forbidden").into_response(),
            AppError::Db(e) => {
                eprintln!("db error: {e}");
                (StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong").into_response()
            }
        }
    }
}

// --- Helpers --------------------------------------------------------------------

fn base_url(state: &AppState, headers: &HeaderMap) -> String {
    if let Some(b) = &state.base_url {
        return b.trim_end_matches('/').to_string();
    }
    let host = headers
        .get(HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("localhost:3000");
    format!("http://{host}")
}

fn set_token_cookie(jar: CookieJar, group_id: &str, token: &str, secure: bool) -> CookieJar {
    let mut c = Cookie::new(ids::cookie_name(group_id), token.to_string());
    c.set_path("/");
    c.set_http_only(true);
    c.set_same_site(SameSite::Lax);
    c.set_secure(secure);
    c.set_max_age(time::Duration::days(365));
    jar.add(c)
}

/// Resolve the current member for a group from the request's cookie.
async fn current_member(
    pool: &SqlitePool,
    jar: &CookieJar,
    group_id: &str,
) -> Result<Option<crate::models::Member>, sqlx::Error> {
    match jar.get(&ids::cookie_name(group_id)) {
        Some(c) => db::member_by_token(pool, group_id, c.value()).await,
        None => Ok(None),
    }
}

fn name_map(members: &[crate::models::Member]) -> HashMap<i64, String> {
    members.iter().map(|m| (m.id, m.name.clone())).collect()
}

// --- Landing / create -----------------------------------------------------------

pub async fn landing() -> Markup {
    views::landing()
}

#[derive(Deserialize)]
pub struct CreateForm {
    name: String,
    your_name: String,
    currency: Option<String>,
}

pub async fn create_group(
    State(st): State<AppState>,
    jar: CookieJar,
    axum::Form(form): axum::Form<CreateForm>,
) -> Result<(CookieJar, Redirect), AppError> {
    let name = form.name.trim();
    let your_name = form.your_name.trim();
    if name.is_empty() || your_name.is_empty() {
        return Ok((jar, Redirect::to("/")));
    }
    let currency = form
        .currency
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .unwrap_or("SEK")
        .to_uppercase();

    let gid = ids::group_id();
    let token = ids::device_token();

    let mut tx = st.pool.begin().await?;
    sqlx::query("INSERT INTO groups (id, name, currency) VALUES (?, ?, ?)")
        .bind(&gid)
        .bind(name)
        .bind(&currency)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO members (group_id, name, token_hash, is_owner) VALUES (?, ?, ?, 1)",
    )
    .bind(&gid)
    .bind(your_name)
    .bind(ids::hash_token(&token))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let jar = set_token_cookie(jar, &gid, &token, st.secure_cookies);
    Ok((jar, Redirect::to(&format!("/g/{gid}"))))
}

// --- Group page -----------------------------------------------------------------

pub async fn group_page(
    State(st): State<AppState>,
    Path(gid): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Markup, AppError> {
    let group = db::load_group(&st.pool, &gid)
        .await?
        .ok_or(AppError::NotFound)?;

    let me = match current_member(&st.pool, &jar, &gid).await? {
        Some(m) => m,
        None => return Ok(views::claim(&group)),
    };

    let members = db::list_members(&st.pool, &gid).await?;
    let names = name_map(&members);
    let member_ids: Vec<i64> = members.iter().map(|m| m.id).collect();

    // Balances + suggested transfers.
    let payments = db::expense_payments(&st.pool, &gid).await?;
    let shares = db::expense_share_rows(&st.pool, &gid).await?;
    let settle_rows = db::settlement_rows(&st.pool, &gid).await?;
    let balances = settle::net_balances(&member_ids, &payments, &shares, &settle_rows);
    let transfers = settle::simplify(&balances);

    let balance_rows: Vec<views::BalanceRow> = balances
        .iter()
        .map(|&(id, net)| views::BalanceRow {
            name: names.get(&id).cloned().unwrap_or_default(),
            net,
        })
        .collect();
    let transfer_rows: Vec<views::TransferRow> = transfers
        .iter()
        .map(|t| views::TransferRow {
            from_id: t.from,
            from: names.get(&t.from).cloned().unwrap_or_default(),
            to_id: t.to,
            to: names.get(&t.to).cloned().unwrap_or_default(),
            amount: t.amount,
        })
        .collect();

    // Expense log.
    let expenses = db::list_expenses(&st.pool, &gid).await?;
    let mut expense_rows = Vec::with_capacity(expenses.len());
    for e in &expenses {
        let participant_ids = db::expense_participants(&st.pool, e.id).await?;
        let participants = participant_ids
            .iter()
            .filter_map(|id| names.get(id).cloned())
            .collect::<Vec<_>>()
            .join(", ");
        expense_rows.push(views::ExpenseRow {
            id: e.id,
            payer: names.get(&e.payer_id).cloned().unwrap_or_default(),
            amount: e.amount,
            description: e.description.clone(),
            participants,
            created_at: e.created_at.clone(),
            can_delete: e.payer_id == me.id || me.is_owner,
        });
    }

    // Settlement log.
    let settlements = db::list_settlements(&st.pool, &gid).await?;
    let settlement_rows: Vec<views::SettlementRow> = settlements
        .iter()
        .map(|s| views::SettlementRow {
            from: names.get(&s.from_id).cloned().unwrap_or_default(),
            to: names.get(&s.to_id).cloned().unwrap_or_default(),
            amount: s.amount,
            created_at: s.created_at.clone(),
        })
        .collect();

    let join_url = format!("{}/g/{}", base_url(&st, &headers), gid);
    let member_rows: Vec<views::MemberRow> = members
        .iter()
        .map(|m| views::MemberRow {
            id: m.id,
            name: m.name.clone(),
            is_owner: m.is_owner,
        })
        .collect();

    let view = GroupView {
        group: &group,
        me: &me,
        join_url: &join_url,
        members: member_rows,
        balances: balance_rows,
        transfers: transfer_rows,
        expenses: expense_rows,
        settlements: settlement_rows,
    };
    Ok(views::group_page(&view))
}

// --- Join -----------------------------------------------------------------------

#[derive(Deserialize)]
pub struct JoinForm {
    name: String,
}

pub async fn join_group(
    State(st): State<AppState>,
    Path(gid): Path<String>,
    jar: CookieJar,
    axum::Form(form): axum::Form<JoinForm>,
) -> Result<(CookieJar, Redirect), AppError> {
    let group = db::load_group(&st.pool, &gid)
        .await?
        .ok_or(AppError::NotFound)?;
    let name = form.name.trim();
    if name.is_empty() {
        return Ok((jar, Redirect::to(&format!("/g/{gid}"))));
    }
    // Already a member on this device? Just go in.
    if current_member(&st.pool, &jar, &gid).await?.is_some() {
        return Ok((jar, Redirect::to(&format!("/g/{gid}"))));
    }
    let token = ids::device_token();
    sqlx::query("INSERT INTO members (group_id, name, token_hash, is_owner) VALUES (?, ?, ?, 0)")
        .bind(&group.id)
        .bind(name)
        .bind(ids::hash_token(&token))
        .execute(&st.pool)
        .await?;
    db::touch_group(&st.pool, &gid).await?;
    let jar = set_token_cookie(jar, &gid, &token, st.secure_cookies);
    Ok((jar, Redirect::to(&format!("/g/{gid}"))))
}

// --- Add / delete expense -------------------------------------------------------

pub async fn add_expense(
    State(st): State<AppState>,
    Path(gid): Path<String>,
    jar: CookieJar,
    body: String,
) -> Result<Redirect, AppError> {
    let group = db::load_group(&st.pool, &gid)
        .await?
        .ok_or(AppError::NotFound)?;
    // Must be a member, group must be open.
    current_member(&st.pool, &jar, &gid)
        .await?
        .ok_or(AppError::Forbidden)?;
    let back = Redirect::to(&format!("/g/{gid}"));
    if group.is_closed() {
        return Ok(back);
    }

    let fields: Vec<(String, String)> = serde_urlencoded::from_str(&body).unwrap_or_default();
    let get = |key: &str| {
        fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };

    let members = db::list_members(&st.pool, &gid).await?;
    let member_ids: std::collections::HashSet<i64> = members.iter().map(|m| m.id).collect();

    let payer_id: i64 = match get("payer_id").and_then(|s| s.parse().ok()) {
        Some(p) if member_ids.contains(&p) => p,
        _ => return Ok(back),
    };
    let description = get("description").unwrap_or("").trim().to_string();
    let method = get("method").unwrap_or("equal");

    let shares: Vec<(i64, i64)> = if method == "exact" {
        let mut v = Vec::new();
        for (k, val) in &fields {
            if let Some(idstr) = k.strip_prefix("amt_") {
                if let Ok(id) = idstr.parse::<i64>() {
                    if member_ids.contains(&id) {
                        if let Some(a) = parse_amount(val) {
                            if a > 0 {
                                v.push((id, a));
                            }
                        }
                    }
                }
            }
        }
        v
    } else {
        let mut included = Vec::new();
        for (k, _) in &fields {
            if let Some(idstr) = k.strip_prefix("inc_") {
                if let Ok(id) = idstr.parse::<i64>() {
                    if member_ids.contains(&id) {
                        included.push(id);
                    }
                }
            }
        }
        included.sort();
        let total = get("amount").and_then(parse_amount).unwrap_or(0);
        equal_shares(total, &included)
    };

    let total: i64 = shares.iter().map(|(_, a)| a).sum();
    if shares.is_empty() || total <= 0 || description.is_empty() {
        return Ok(back);
    }

    let mut tx = st.pool.begin().await?;
    let eid: i64 = sqlx::query_scalar(
        "INSERT INTO expenses (group_id, payer_id, amount, description)
         VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(&gid)
    .bind(payer_id)
    .bind(total)
    .bind(&description)
    .fetch_one(&mut *tx)
    .await?;
    for (mid, amt) in &shares {
        sqlx::query("INSERT INTO expense_shares (expense_id, member_id, amount) VALUES (?, ?, ?)")
            .bind(eid)
            .bind(mid)
            .bind(amt)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE groups SET last_active = datetime('now') WHERE id = ?")
        .bind(&gid)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(back)
}

pub async fn delete_expense(
    State(st): State<AppState>,
    Path((gid, eid)): Path<(String, i64)>,
    jar: CookieJar,
) -> Result<Redirect, AppError> {
    let me = current_member(&st.pool, &jar, &gid)
        .await?
        .ok_or(AppError::Forbidden)?;
    let back = Redirect::to(&format!("/g/{gid}"));
    // Only the payer or the owner may delete.
    let payer: Option<(i64,)> =
        sqlx::query_as("SELECT payer_id FROM expenses WHERE id = ? AND group_id = ? AND deleted_at IS NULL")
            .bind(eid)
            .bind(&gid)
            .fetch_optional(&st.pool)
            .await?;
    let Some((payer_id,)) = payer else {
        return Ok(back);
    };
    if payer_id != me.id && !me.is_owner {
        return Err(AppError::Forbidden);
    }
    sqlx::query("UPDATE expenses SET deleted_at = datetime('now') WHERE id = ?")
        .bind(eid)
        .execute(&st.pool)
        .await?;
    db::touch_group(&st.pool, &gid).await?;
    Ok(back)
}

// --- Settlements ----------------------------------------------------------------

#[derive(Deserialize)]
pub struct SettlementForm {
    from_id: i64,
    to_id: i64,
    amount_ore: i64,
}

pub async fn mark_settlement(
    State(st): State<AppState>,
    Path(gid): Path<String>,
    jar: CookieJar,
    axum::Form(form): axum::Form<SettlementForm>,
) -> Result<Redirect, AppError> {
    let group = db::load_group(&st.pool, &gid)
        .await?
        .ok_or(AppError::NotFound)?;
    current_member(&st.pool, &jar, &gid)
        .await?
        .ok_or(AppError::Forbidden)?;
    let back = Redirect::to(&format!("/g/{gid}"));
    if group.is_closed() || form.amount_ore <= 0 || form.from_id == form.to_id {
        return Ok(back);
    }
    // Both parties must belong to the group.
    let members = db::list_members(&st.pool, &gid).await?;
    let ids: std::collections::HashSet<i64> = members.iter().map(|m| m.id).collect();
    if !ids.contains(&form.from_id) || !ids.contains(&form.to_id) {
        return Ok(back);
    }
    // Clamp to the actual outstanding debt so a double-tap (two people marking the
    // same suggested payment) can't overshoot and invent a reverse debt.
    let member_ids: Vec<i64> = members.iter().map(|m| m.id).collect();
    let payments = db::expense_payments(&st.pool, &gid).await?;
    let shares = db::expense_share_rows(&st.pool, &gid).await?;
    let settle_rows = db::settlement_rows(&st.pool, &gid).await?;
    let balances: HashMap<i64, i64> =
        settle::net_balances(&member_ids, &payments, &shares, &settle_rows)
            .into_iter()
            .collect();
    let owes = (-balances.get(&form.from_id).copied().unwrap_or(0)).max(0);
    let owed = balances.get(&form.to_id).copied().unwrap_or(0).max(0);
    let amount = form.amount_ore.min(owes).min(owed);
    if amount <= 0 {
        return Ok(back);
    }
    sqlx::query(
        "INSERT INTO settlements (group_id, from_id, to_id, amount) VALUES (?, ?, ?, ?)",
    )
    .bind(&gid)
    .bind(form.from_id)
    .bind(form.to_id)
    .bind(amount)
    .execute(&st.pool)
    .await?;
    db::touch_group(&st.pool, &gid).await?;
    Ok(back)
}

// --- Owner: close / reopen / recovery -------------------------------------------

async fn require_owner(
    pool: &SqlitePool,
    jar: &CookieJar,
    gid: &str,
) -> Result<crate::models::Member, AppError> {
    let me = current_member(pool, jar, gid)
        .await?
        .ok_or(AppError::Forbidden)?;
    if !me.is_owner {
        return Err(AppError::Forbidden);
    }
    Ok(me)
}

pub async fn close_group(
    State(st): State<AppState>,
    Path(gid): Path<String>,
    jar: CookieJar,
) -> Result<Redirect, AppError> {
    require_owner(&st.pool, &jar, &gid).await?;
    sqlx::query("UPDATE groups SET closed_at = datetime('now') WHERE id = ?")
        .bind(&gid)
        .execute(&st.pool)
        .await?;
    Ok(Redirect::to(&format!("/g/{gid}")))
}

pub async fn reopen_group(
    State(st): State<AppState>,
    Path(gid): Path<String>,
    jar: CookieJar,
) -> Result<Redirect, AppError> {
    require_owner(&st.pool, &jar, &gid).await?;
    sqlx::query("UPDATE groups SET closed_at = NULL, last_active = datetime('now') WHERE id = ?")
        .bind(&gid)
        .execute(&st.pool)
        .await?;
    Ok(Redirect::to(&format!("/g/{gid}")))
}

#[derive(Deserialize)]
pub struct RecoveryForm {
    passphrase: String,
}

pub async fn set_recovery(
    State(st): State<AppState>,
    Path(gid): Path<String>,
    jar: CookieJar,
    axum::Form(form): axum::Form<RecoveryForm>,
) -> Result<Redirect, AppError> {
    require_owner(&st.pool, &jar, &gid).await?;
    let pass = form.passphrase.trim();
    let back = Redirect::to(&format!("/g/{gid}"));
    if pass.is_empty() {
        return Ok(back);
    }
    // Stored as an unsalted SHA-256 hash — adequate for this low-value recovery
    // secret in v1; swap for a proper KDF (argon2) if this ever guards real value.
    sqlx::query("UPDATE groups SET recovery = ? WHERE id = ?")
        .bind(ids::hash_token(pass))
        .bind(&gid)
        .execute(&st.pool)
        .await?;
    Ok(back)
}

pub async fn recover_page(
    State(st): State<AppState>,
    Path(gid): Path<String>,
) -> Result<Markup, AppError> {
    let group = db::load_group(&st.pool, &gid)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(views::recover(&group, false))
}

pub async fn recover_submit(
    State(st): State<AppState>,
    Path(gid): Path<String>,
    jar: CookieJar,
    axum::Form(form): axum::Form<RecoveryForm>,
) -> Result<Response, AppError> {
    let group = db::load_group(&st.pool, &gid)
        .await?
        .ok_or(AppError::NotFound)?;
    let matches = group
        .recovery
        .as_deref()
        .map(|stored| stored == ids::hash_token(form.passphrase.trim()))
        .unwrap_or(false);
    if !matches {
        return Ok(views::recover(&group, true).into_response());
    }
    // Rotate the owner's device token onto this device.
    let token = ids::device_token();
    sqlx::query("UPDATE members SET token_hash = ? WHERE group_id = ? AND is_owner = 1")
        .bind(ids::hash_token(&token))
        .bind(&gid)
        .execute(&st.pool)
        .await?;
    let jar = set_token_cookie(jar, &gid, &token, st.secure_cookies);
    Ok((jar, Redirect::to(&format!("/g/{gid}"))).into_response())
}
