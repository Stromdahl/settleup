//! HTTP route handlers.

use axum::extract::{Path, Query, State};
use axum::http::header::HOST;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
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
        None => {
            // Not a member yet: show the claim/join screen with a little social proof
            // (who started it, how many are in, the running tab) — all read-only, and
            // anyone with the link can join and see it anyway.
            let members = db::list_members(&st.pool, &gid).await?;
            let member_rows: Vec<views::MemberRow> = members
                .iter()
                .map(|m| views::MemberRow {
                    id: m.id,
                    name: m.name.clone(),
                    is_owner: m.is_owner,
                })
                .collect();
            let total: i64 = db::list_expenses(&st.pool, &gid)
                .await?
                .iter()
                .map(|e| e.amount)
                .sum();
            return Ok(views::claim(&group, &member_rows, total));
        }
    };

    let members = db::list_members(&st.pool, &gid).await?;
    let ledger = build_ledger(&st.pool, &gid, &members, &me).await?;
    let version = db::group_version(&st.pool, &gid).await?;

    let join_url = format!("{}/g/{}", base_url(&st, &headers), gid);
    let view = group_view(&group, &me, &join_url, &members, ledger, version);
    Ok(views::group_page(&view))
}

/// The focused "New expense" screen (frame 04). Members of an open group only; a closed
/// group or a non-member is bounced back to the group page.
pub async fn add_expense_page(
    State(st): State<AppState>,
    Path(gid): Path<String>,
    jar: CookieJar,
) -> Result<Response, AppError> {
    let group = db::load_group(&st.pool, &gid)
        .await?
        .ok_or(AppError::NotFound)?;
    let me = current_member(&st.pool, &jar, &gid)
        .await?
        .ok_or(AppError::Forbidden)?;
    if group.is_closed() {
        return Ok(Redirect::to(&format!("/g/{gid}")).into_response());
    }
    let members = db::list_members(&st.pool, &gid).await?;
    let member_rows: Vec<views::MemberRow> = members
        .iter()
        .map(|m| views::MemberRow {
            id: m.id,
            name: m.name.clone(),
            is_owner: m.is_owner,
        })
        .collect();
    Ok(views::add_expense_page(&group, &me, &member_rows).into_response())
}

/// The rendered ledger for a group: net balances, the simplified transfers that settle
/// it, and the expense + settlement logs. Shared by the full page and the live poll so
/// the two can't drift.
struct Ledger {
    balances: Vec<views::BalanceRow>,
    transfers: Vec<views::TransferRow>,
    expenses: Vec<views::ExpenseRow>,
    settlements: Vec<views::SettlementRow>,
}

async fn build_ledger(
    pool: &SqlitePool,
    gid: &str,
    members: &[crate::models::Member],
    me: &crate::models::Member,
) -> Result<Ledger, sqlx::Error> {
    let names = name_map(members);
    let member_ids: Vec<i64> = members.iter().map(|m| m.id).collect();

    // Balances + suggested transfers.
    let payments = db::expense_payments(pool, gid).await?;
    let shares = db::expense_share_rows(pool, gid).await?;
    let settle_rows = db::settlement_rows(pool, gid).await?;
    let balances = settle::net_balances(&member_ids, &payments, &shares, &settle_rows);
    let transfers = settle::simplify(&balances);

    let owner_id = members.iter().find(|m| m.is_owner).map(|m| m.id);
    let balance_rows: Vec<views::BalanceRow> = balances
        .iter()
        .map(|&(id, net)| views::BalanceRow {
            id,
            name: names.get(&id).cloned().unwrap_or_default(),
            net,
            is_owner: Some(id) == owner_id,
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
    let expenses = db::list_expenses(pool, gid).await?;
    let mut expense_rows = Vec::with_capacity(expenses.len());
    for e in &expenses {
        let participant_ids = db::expense_participants(pool, e.id).await?;
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
    let settlements = db::list_settlements(pool, gid).await?;
    let settlement_rows: Vec<views::SettlementRow> = settlements
        .iter()
        .map(|s| views::SettlementRow {
            from: names.get(&s.from_id).cloned().unwrap_or_default(),
            to: names.get(&s.to_id).cloned().unwrap_or_default(),
            amount: s.amount,
            created_at: s.created_at.clone(),
        })
        .collect();

    Ok(Ledger {
        balances: balance_rows,
        transfers: transfer_rows,
        expenses: expense_rows,
        settlements: settlement_rows,
    })
}

/// Assemble a [`GroupView`] from a group, its members, and a freshly-built ledger.
fn group_view<'a>(
    group: &'a crate::models::Group,
    me: &'a crate::models::Member,
    join_url: &'a str,
    members: &[crate::models::Member],
    ledger: Ledger,
    version: i64,
) -> GroupView<'a> {
    let member_rows: Vec<views::MemberRow> = members
        .iter()
        .map(|m| views::MemberRow {
            id: m.id,
            name: m.name.clone(),
            is_owner: m.is_owner,
        })
        .collect();
    GroupView {
        group,
        me,
        join_url,
        members: member_rows,
        balances: ledger.balances,
        transfers: ledger.transfers,
        expenses: ledger.expenses,
        settlements: ledger.settlements,
        version,
    }
}

// --- Live updates ---------------------------------------------------------------

#[derive(Deserialize)]
pub struct LiveQuery {
    /// Last-seen change token (see [`db::group_version`]).
    v: Option<i64>,
    /// Last-seen member count (to distinguish a join from an expense/settlement).
    m: Option<i64>,
    /// Last-seen closed flag, 0 or 1 (to catch close/reopen).
    c: Option<i64>,
}

/// The 5-second poll behind live updates. Returns, in decreasing order of cheapness:
/// `HX-Refresh` when the group was closed/reopened (a structural change the read-only
/// fragments can't express — rebuild the whole page); `204 No Content` when nothing
/// changed; or out-of-band fragment swaps for the read-only regions (plus a "someone
/// joined" notice when the roster grew). The add-expense form is never sent, so
/// in-progress input is safe. See decision #10 in docs/DECISIONS.md.
pub async fn live(
    State(st): State<AppState>,
    Path(gid): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(q): Query<LiveQuery>,
) -> Result<Response, AppError> {
    let group = db::load_group(&st.pool, &gid)
        .await?
        .ok_or(AppError::NotFound)?;
    // Same visibility rule as the full page: members only.
    let me = current_member(&st.pool, &jar, &gid)
        .await?
        .ok_or(AppError::Forbidden)?;

    let closed = i64::from(group.is_closed());
    let version = db::group_version(&st.pool, &gid).await?;

    // Structural change (close/reopen): the form, FAB, and banner all appear/disappear,
    // so a clean full reload is the honest fix. Only fires when the client had a prior
    // state to compare against.
    if q.c.is_some() && q.c != Some(closed) {
        let mut resp = StatusCode::OK.into_response();
        resp.headers_mut()
            .insert("HX-Refresh", HeaderValue::from_static("true"));
        return Ok(resp);
    }

    // Nothing changed since the client last rendered — swap nothing.
    if q.v == Some(version) {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }

    // A data change (expense/settlement) or a join: repaint the read-only regions.
    let members = db::list_members(&st.pool, &gid).await?;
    let member_count = members.len() as i64;
    let joined = q.m.is_some_and(|m| member_count > m);

    let ledger = build_ledger(&st.pool, &gid, &members, &me).await?;
    let join_url = format!("{}/g/{}", base_url(&st, &headers), gid);
    let view = group_view(&group, &me, &join_url, &members, ledger, version);
    Ok(views::live_update(&view, joined).into_response())
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

#[cfg(test)]
mod tests {
    //! End-to-end tests that drive `add_expense` against a real (in-memory) DB.
    //! The simulation campaign validates the settle math the handler *feeds*; these
    //! close the loop by checking the handler actually parses, computes, validates,
    //! and persists the right rows — that `amount` equals the sum of shares, that
    //! equal/exact splits store what they should, and that bad input persists nothing.
    use super::*;
    use axum_extra::extract::cookie::Cookie;

    /// A group with Alice (owner, authenticated via the returned token), Bob, Cara.
    async fn group_with_three(pool: &SqlitePool) -> (String, [i64; 3], String) {
        let gid = "g".to_string();
        let token = "alice-device-token".to_string();
        sqlx::query("INSERT INTO groups (id, name) VALUES (?, 'Trip')")
            .bind(&gid).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO members (group_id, name, token_hash, is_owner) VALUES (?, 'Alice', ?, 1)")
            .bind(&gid).bind(ids::hash_token(&token)).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO members (group_id, name, token_hash) VALUES (?, 'Bob', 'hb')")
            .bind(&gid).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO members (group_id, name, token_hash) VALUES (?, 'Cara', 'hc')")
            .bind(&gid).execute(pool).await.unwrap();
        let ids: Vec<i64> = sqlx::query_as::<_, (i64,)>(
            "SELECT id FROM members WHERE group_id = ? ORDER BY id",
        )
        .bind(&gid).fetch_all(pool).await.unwrap()
        .into_iter().map(|(x,)| x).collect();
        (gid, [ids[0], ids[1], ids[2]], token)
    }

    fn state(pool: SqlitePool) -> AppState {
        AppState { pool, base_url: None, secure_cookies: false }
    }

    fn auth_jar(gid: &str, token: &str) -> CookieJar {
        CookieJar::new().add(Cookie::new(ids::cookie_name(gid), token.to_string()))
    }

    /// `(stored amount, sorted shares)` for the group's one live expense.
    async fn persisted(pool: &SqlitePool, gid: &str) -> (i64, Vec<(i64, i64)>) {
        let amount: i64 = sqlx::query_scalar(
            "SELECT amount FROM expenses WHERE group_id = ? AND deleted_at IS NULL",
        )
        .bind(gid).fetch_one(pool).await.unwrap();
        let mut shares = db::expense_share_rows(pool, gid).await.unwrap();
        shares.sort();
        (amount, shares)
    }

    async fn expense_count(pool: &SqlitePool, gid: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM expenses WHERE group_id = ?")
            .bind(gid).fetch_one(pool).await.unwrap()
    }

    #[tokio::test]
    async fn equal_split_persists_equal_shares_and_matching_total() {
        let pool = db::memory_pool().await;
        let (gid, [alice, bob, cara], token) = group_with_three(&pool).await;

        // 100.00 split equally among all three.
        let body = format!(
            "payer_id={alice}&description=Dinner&method=equal&amount=100&inc_{alice}=on&inc_{bob}=on&inc_{cara}=on"
        );
        let _ = add_expense(State(state(pool.clone())), Path(gid.clone()), auth_jar(&gid, &token), body)
            .await.map_err(|_| "handler returned an error").unwrap();

        // 10000 öre / 3 = 3334, 3333, 3333 — the leftover öre goes to the lowest id.
        let (amount, shares) = persisted(&pool, &gid).await;
        assert_eq!(shares, vec![(alice, 3334), (bob, 3333), (cara, 3333)]);
        assert_eq!(amount, 10000);
        assert_eq!(amount, shares.iter().map(|(_, a)| a).sum::<i64>(), "stored total must equal Σ shares");
    }

    #[tokio::test]
    async fn exact_split_persists_submitted_amounts_and_conserves_money() {
        let pool = db::memory_pool().await;
        let (gid, [alice, bob, cara], token) = group_with_three(&pool).await;

        // Alice fronts it; Bob owes 80.00, Cara owes 40.50; Alice isn't splitting.
        let body = format!("payer_id={alice}&description=Nachos&method=exact&amt_{bob}=80&amt_{cara}=40.50");
        let _ = add_expense(State(state(pool.clone())), Path(gid.clone()), auth_jar(&gid, &token), body)
            .await.map_err(|_| "handler returned an error").unwrap();

        let (amount, shares) = persisted(&pool, &gid).await;
        assert_eq!(shares, vec![(bob, 8000), (cara, 4050)]);
        assert_eq!(amount, 12050, "exact total is the sum of the entered amounts");

        // Money conserves end-to-end, through the real balance queries.
        let members = vec![alice, bob, cara];
        let bal = settle::net_balances(
            &members,
            &db::expense_payments(&pool, &gid).await.unwrap(),
            &db::expense_share_rows(&pool, &gid).await.unwrap(),
            &db::settlement_rows(&pool, &gid).await.unwrap(),
        );
        assert_eq!(bal, vec![(alice, 12050), (bob, -8000), (cara, -4050)]);
        assert_eq!(bal.iter().map(|(_, b)| b).sum::<i64>(), 0);
    }

    #[tokio::test]
    async fn invalid_inputs_persist_nothing() {
        let pool = db::memory_pool().await;
        let (gid, [alice, bob, _cara], token) = group_with_three(&pool).await;
        let send = |body: String| {
            let (p, g, t) = (pool.clone(), gid.clone(), token.clone());
            async move { add_expense(State(state(p)), Path(g.clone()), auth_jar(&g, &t), body).await }
        };

        // Blank description.
        let _ = send(format!("payer_id={alice}&description=&method=equal&amount=50&inc_{alice}=on")).await;
        // Zero total.
        let _ = send(format!("payer_id={alice}&description=X&method=equal&amount=0&inc_{alice}=on")).await;
        // No participants selected.
        let _ = send(format!("payer_id={alice}&description=X&method=equal&amount=50")).await;
        // Exact split with only a non-positive amount.
        let _ = send(format!("payer_id={alice}&description=X&method=exact&amt_{bob}=0")).await;
        // Payer isn't a member of the group.
        let _ = send(format!("payer_id=999999&description=X&method=equal&amount=50&inc_{alice}=on")).await;

        assert_eq!(expense_count(&pool, &gid).await, 0, "no invalid submission should persist");
    }

    #[tokio::test]
    async fn unauthenticated_is_rejected_and_persists_nothing() {
        let pool = db::memory_pool().await;
        let (gid, [alice, _bob, _cara], _token) = group_with_three(&pool).await;
        let body = format!("payer_id={alice}&description=X&method=equal&amount=50&inc_{alice}=on");
        // No device cookie → Forbidden.
        let res = add_expense(State(state(pool.clone())), Path(gid.clone()), CookieJar::new(), body).await;
        assert!(res.is_err(), "a request with no device cookie must be rejected");
        assert_eq!(expense_count(&pool, &gid).await, 0);
    }

    #[tokio::test]
    async fn closed_group_rejects_new_expenses() {
        let pool = db::memory_pool().await;
        let (gid, [alice, bob, cara], token) = group_with_three(&pool).await;
        sqlx::query("UPDATE groups SET closed_at = datetime('now') WHERE id = ?")
            .bind(&gid).execute(&pool).await.unwrap();

        let body = format!(
            "payer_id={alice}&description=Late&method=equal&amount=90&inc_{alice}=on&inc_{bob}=on&inc_{cara}=on"
        );
        let _ = add_expense(State(state(pool.clone())), Path(gid.clone()), auth_jar(&gid, &token), body).await;
        assert_eq!(expense_count(&pool, &gid).await, 0, "a closed group must not accept new expenses");
    }

    // --- Add-expense screen (GET /g/{id}/add) -------------------------------------

    #[tokio::test]
    async fn add_expense_page_renders_the_form_for_a_member() {
        let pool = db::memory_pool().await;
        let (gid, [_alice, bob, cara], token) = group_with_three(&pool).await;
        let resp = add_expense_page(State(state(pool.clone())), Path(gid.clone()), auth_jar(&gid, &token))
            .await.map_err(|_| "member should see the form").unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let out = body_string(resp).await;
        assert!(out.contains("New expense"), "the focused screen has its heading");
        assert!(out.contains(&format!(r#"action="/g/{gid}/expenses""#)), "posts to the expense endpoint");
        // Every member is a selectable participant on this screen.
        assert!(out.contains(&format!(r#"name="inc_{bob}""#)) && out.contains(&format!(r#"name="inc_{cara}""#)));
    }

    #[tokio::test]
    async fn add_expense_page_forbidden_for_non_member() {
        let pool = db::memory_pool().await;
        let (gid, _ids, _token) = group_with_three(&pool).await;
        let res = add_expense_page(State(state(pool.clone())), Path(gid.clone()), CookieJar::new()).await;
        assert!(res.is_err(), "a non-member must not reach the add-expense screen");
    }

    #[tokio::test]
    async fn add_expense_page_redirects_when_closed() {
        let pool = db::memory_pool().await;
        let (gid, _ids, token) = group_with_three(&pool).await;
        sqlx::query("UPDATE groups SET closed_at = datetime('now') WHERE id = ?")
            .bind(&gid).execute(&pool).await.unwrap();
        let resp = add_expense_page(State(state(pool.clone())), Path(gid.clone()), auth_jar(&gid, &token))
            .await.map_err(|_| "closed group should redirect, not error").unwrap();
        assert!(resp.status().is_redirection(), "a closed group bounces back to the group page");
        assert_eq!(resp.headers().get("location").unwrap(), &format!("/g/{gid}"));
    }

    // --- Live-update poll ---------------------------------------------------------

    async fn body_string(resp: Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn q(v: Option<i64>, m: Option<i64>, c: Option<i64>) -> Query<LiveQuery> {
        Query(LiveQuery { v, m, c })
    }

    async fn poll(pool: &SqlitePool, gid: &str, token: &str, q: Query<LiveQuery>) -> Response {
        live(
            State(state(pool.clone())),
            Path(gid.to_string()),
            HeaderMap::new(),
            auth_jar(gid, token),
            q,
        )
        .await
        .map_err(|_| "live poll should not error for a member")
        .unwrap()
    }

    #[tokio::test]
    async fn live_unchanged_returns_204() {
        let pool = db::memory_pool().await;
        let (gid, _ids, token) = group_with_three(&pool).await;
        let version = db::group_version(&pool, &gid).await.unwrap();

        // Client is fully up to date: current token, 3 members, open.
        let resp = poll(&pool, &gid, &token, q(Some(version), Some(3), Some(0))).await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT, "no change → swap nothing");
    }

    #[tokio::test]
    async fn live_expense_change_sends_oob_fragments_without_notice() {
        let pool = db::memory_pool().await;
        let (gid, [alice, bob, cara], token) = group_with_three(&pool).await;
        let body = format!(
            "payer_id={alice}&description=Dinner&method=equal&amount=100&inc_{alice}=on&inc_{bob}=on&inc_{cara}=on"
        );
        let _ = add_expense(State(state(pool.clone())), Path(gid.clone()), auth_jar(&gid, &token), body)
            .await.map_err(|_| "add failed").unwrap();

        // Client saw an older token but the same 3-member, open roster.
        let resp = poll(&pool, &gid, &token, q(Some(0), Some(3), Some(0))).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let out = body_string(resp).await;
        assert!(out.contains("hx-swap-oob"), "must carry out-of-band swaps");
        assert!(out.contains(r#"id="ls-ledger""#), "read-only ledger region is swapped");
        assert!(out.contains("Dinner"), "the new expense appears in the swapped ledger");
        assert!(out.contains(r#"id="ls-poll""#), "poller token is re-sent so the guard advances");
        assert!(!out.contains("Someone just joined"), "an expense is not a join");
        assert!(!out.contains(r#"name="amount""#), "the add-expense form is never sent");
    }

    #[tokio::test]
    async fn live_join_includes_the_notice() {
        let pool = db::memory_pool().await;
        let (gid, _ids, token) = group_with_three(&pool).await;
        // A fourth person joins.
        sqlx::query("INSERT INTO members (group_id, name, token_hash) VALUES (?, 'Dave', 'hd')")
            .bind(&gid).execute(&pool).await.unwrap();
        db::touch_group(&pool, &gid).await.unwrap();

        // Client still thinks there are 3 members.
        let resp = poll(&pool, &gid, &token, q(Some(0), Some(3), Some(0))).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let out = body_string(resp).await;
        assert!(out.contains("Someone just joined"), "a roster growth shows the join notice");
        assert!(out.contains("Refresh to include them"));
    }

    #[tokio::test]
    async fn live_close_and_reopen_trigger_full_reload() {
        let pool = db::memory_pool().await;
        let (gid, _ids, token) = group_with_three(&pool).await;

        // Owner closes; a viewer who last saw it open must be told to reload.
        sqlx::query("UPDATE groups SET closed_at = datetime('now') WHERE id = ?")
            .bind(&gid).execute(&pool).await.unwrap();
        let resp = poll(&pool, &gid, &token, q(Some(0), Some(3), Some(0))).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get("HX-Refresh").unwrap(), "true", "close → reload");

        // And a viewer who last saw it closed must be told to reload on reopen.
        sqlx::query("UPDATE groups SET closed_at = NULL WHERE id = ?")
            .bind(&gid).execute(&pool).await.unwrap();
        let resp = poll(&pool, &gid, &token, q(Some(0), Some(3), Some(1))).await;
        assert_eq!(resp.headers().get("HX-Refresh").unwrap(), "true", "reopen → reload");
    }

    #[tokio::test]
    async fn live_closed_and_current_does_not_loop_on_refresh() {
        // A closed group the client already knows is closed, with nothing new, must
        // return 204 — never a fresh HX-Refresh, or open pages would reload forever.
        let pool = db::memory_pool().await;
        let (gid, _ids, token) = group_with_three(&pool).await;
        sqlx::query("UPDATE groups SET closed_at = datetime('now') WHERE id = ?")
            .bind(&gid).execute(&pool).await.unwrap();
        let version = db::group_version(&pool, &gid).await.unwrap();

        let resp = poll(&pool, &gid, &token, q(Some(version), Some(3), Some(1))).await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(resp.headers().get("HX-Refresh").is_none());
    }

    #[tokio::test]
    async fn live_rejects_non_members() {
        let pool = db::memory_pool().await;
        let (gid, _ids, _token) = group_with_three(&pool).await;
        let res = live(
            State(state(pool.clone())),
            Path(gid.clone()),
            HeaderMap::new(),
            CookieJar::new(),
            q(None, None, None),
        )
        .await;
        assert!(res.is_err(), "a non-member must not receive group fragments");
    }
}
