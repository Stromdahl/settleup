//! HTTP route handlers.

use axum::extract::{Path, Query, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, HOST};
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

#[derive(Debug)]
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

/// May this member manage (edit or delete) an expense paid by `payer_id`? The original
/// payer and the group owner can; nobody else. Callers still distinguish a missing
/// expense (redirect back) from a forbidden one (403) themselves.
fn can_manage(me: &crate::models::Member, payer_id: i64) -> bool {
    payer_id == me.id || me.is_owner
}

/// Project the DB member rows onto the view's [`views::MemberRow`] (id, name, owner flag).
fn member_rows(members: &[crate::models::Member]) -> Vec<views::MemberRow> {
    members
        .iter()
        .map(|m| views::MemberRow {
            id: m.id,
            name: m.name.clone(),
            is_owner: m.is_owner,
        })
        .collect()
}

// --- Static assets --------------------------------------------------------------

/// Vendored htmx, compiled into the binary and served from our own origin so the app
/// carries no third-party script dependency (works offline, and clears the path to a
/// strict CSP). The version is pinned in the route, so the `immutable` year-long cache
/// is safe: bumping htmx changes the URL, which busts client caches for free.
pub async fn htmx_js() -> Response {
    const HTMX: &str = include_str!("../assets/htmx-2.0.4.min.js");
    (
        [
            (CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        HTMX,
    )
        .into_response()
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

    db::create_group_with_owner(
        &st.pool,
        &gid,
        name,
        &currency,
        your_name,
        &ids::hash_token(&token),
    )
    .await?;

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
            let member_rows = member_rows(&members);
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
    let member_rows = member_rows(&members);
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
            can_manage: can_manage(me, e.payer_id),
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
    GroupView {
        group,
        me,
        join_url,
        members: member_rows(members),
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
    db::insert_member(&st.pool, &group.id, name, &ids::hash_token(&token)).await?;
    let jar = set_token_cookie(jar, &gid, &token, st.secure_cookies);
    Ok((jar, Redirect::to(&format!("/g/{gid}"))))
}

// --- Add / edit / delete expense ------------------------------------------------

/// A validated expense submission: `(payer_id, description, shares)`, where `shares` is
/// `(member_id, amount)` in öre.
type ExpenseInput = (i64, String, Vec<(i64, i64)>);

/// Parse the dynamic add/edit expense form body into an [`ExpenseInput`], or `None` if the
/// submission is invalid: unknown payer, empty description, no participating shares, or a
/// non-positive total. Shared by [`add_expense`] and [`edit_expense`] so their parsing and
/// validation can't drift.
///
/// `equal` split reads the `inc_<id>` checkboxes and the single `amount`; `exact` reads
/// each `amt_<id>` field. Unknown member ids are ignored.
fn parse_expense_form(
    body: &str,
    member_ids: &std::collections::HashSet<i64>,
) -> Option<ExpenseInput> {
    let fields: Vec<(String, String)> = serde_urlencoded::from_str(body).unwrap_or_default();
    let get = |key: &str| {
        fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };

    let payer_id: i64 = match get("payer_id").and_then(|s| s.parse().ok()) {
        Some(p) if member_ids.contains(&p) => p,
        _ => return None,
    };
    let description = get("description").unwrap_or("").trim().to_string();
    let method = get("method").unwrap_or("equal");

    let shares: Vec<(i64, i64)> = if method == "exact" {
        let mut v = Vec::new();
        for (k, val) in &fields {
            if let Some(idstr) = k.strip_prefix("amt_")
                && let Ok(id) = idstr.parse::<i64>()
                && member_ids.contains(&id)
                && let Some(a) = parse_amount(val)
                && a > 0
            {
                v.push((id, a));
            }
        }
        v
    } else {
        let mut included = Vec::new();
        for (k, _) in &fields {
            if let Some(idstr) = k.strip_prefix("inc_")
                && let Ok(id) = idstr.parse::<i64>()
                && member_ids.contains(&id)
            {
                included.push(id);
            }
        }
        included.sort();
        let total = get("amount").and_then(parse_amount).unwrap_or(0);
        equal_shares(total, &included)
    };

    let total: i64 = shares.iter().map(|(_, a)| a).sum();
    if shares.is_empty() || total <= 0 || description.is_empty() {
        return None;
    }
    Some((payer_id, description, shares))
}

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

    let members = db::list_members(&st.pool, &gid).await?;
    let member_ids: std::collections::HashSet<i64> = members.iter().map(|m| m.id).collect();
    let Some((payer_id, description, shares)) = parse_expense_form(&body, &member_ids) else {
        return Ok(back);
    };
    let total: i64 = shares.iter().map(|(_, a)| a).sum();

    db::insert_expense_with_shares(&st.pool, &gid, payer_id, total, &description, &shares).await?;
    Ok(back)
}

/// The prefilled edit form for an existing expense. Members-only; redirects back if the
/// group is closed or the expense is gone; `Forbidden` unless the caller is the original
/// payer or the owner (matches [`delete_expense`]).
pub async fn edit_expense_page(
    State(st): State<AppState>,
    Path((gid, eid)): Path<(String, i64)>,
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
    let Some((payer_id, amount, description)) = db::expense_edit_row(&st.pool, &gid, eid).await?
    else {
        return Ok(Redirect::to(&format!("/g/{gid}")).into_response());
    };
    if !can_manage(&me, payer_id) {
        return Err(AppError::Forbidden);
    }

    let members = db::list_members(&st.pool, &gid).await?;
    let member_rows = member_rows(&members);
    let shares = db::expense_shares(&st.pool, eid).await?;
    Ok(views::edit_expense_page(
        &group,
        &member_rows,
        eid,
        payer_id,
        &description,
        amount,
        &shares,
    )
    .into_response())
}

/// Save an edit: replace the expense's payer/total/description and its share snapshot in
/// one transaction. Same parse/validation as [`add_expense`]; balances recompute from the
/// new shares (an edit is deliberately allowed even after settlements — see decision #13).
pub async fn edit_expense(
    State(st): State<AppState>,
    Path((gid, eid)): Path<(String, i64)>,
    jar: CookieJar,
    body: String,
) -> Result<Redirect, AppError> {
    let group = db::load_group(&st.pool, &gid)
        .await?
        .ok_or(AppError::NotFound)?;
    let me = current_member(&st.pool, &jar, &gid)
        .await?
        .ok_or(AppError::Forbidden)?;
    let back = Redirect::to(&format!("/g/{gid}"));
    if group.is_closed() {
        return Ok(back);
    }
    // Expense must exist in this group; permission = original payer or owner.
    let Some(orig_payer) = db::expense_payer(&st.pool, &gid, eid).await? else {
        return Ok(back);
    };
    if !can_manage(&me, orig_payer) {
        return Err(AppError::Forbidden);
    }

    let members = db::list_members(&st.pool, &gid).await?;
    let member_ids: std::collections::HashSet<i64> = members.iter().map(|m| m.id).collect();
    let Some((payer_id, description, shares)) = parse_expense_form(&body, &member_ids) else {
        return Ok(back);
    };
    let total: i64 = shares.iter().map(|(_, a)| a).sum();

    db::update_expense_with_shares(&st.pool, &gid, eid, payer_id, total, &description, &shares)
        .await?;
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
    let Some(payer_id) = db::expense_payer(&st.pool, &gid, eid).await? else {
        return Ok(back);
    };
    if !can_manage(&me, payer_id) {
        return Err(AppError::Forbidden);
    }
    db::soft_delete_expense(&st.pool, &gid, eid).await?;
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
    db::insert_settlement(&st.pool, &gid, form.from_id, form.to_id, amount).await?;
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
    db::close_group(&st.pool, &gid).await?;
    Ok(Redirect::to(&format!("/g/{gid}")))
}

pub async fn reopen_group(
    State(st): State<AppState>,
    Path(gid): Path<String>,
    jar: CookieJar,
) -> Result<Redirect, AppError> {
    require_owner(&st.pool, &jar, &gid).await?;
    db::reopen_group(&st.pool, &gid).await?;
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
    db::set_recovery(&st.pool, &gid, &ids::hash_token(pass)).await?;
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
    db::rotate_owner_token(&st.pool, &gid, &ids::hash_token(&token)).await?;
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
            .bind(&gid)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO members (group_id, name, token_hash, is_owner) VALUES (?, 'Alice', ?, 1)",
        )
        .bind(&gid)
        .bind(ids::hash_token(&token))
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO members (group_id, name, token_hash) VALUES (?, 'Bob', 'hb')")
            .bind(&gid)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO members (group_id, name, token_hash) VALUES (?, 'Cara', 'hc')")
            .bind(&gid)
            .execute(pool)
            .await
            .unwrap();
        let ids: Vec<i64> =
            sqlx::query_as::<_, (i64,)>("SELECT id FROM members WHERE group_id = ? ORDER BY id")
                .bind(&gid)
                .fetch_all(pool)
                .await
                .unwrap()
                .into_iter()
                .map(|(x,)| x)
                .collect();
        (gid, [ids[0], ids[1], ids[2]], token)
    }

    fn state(pool: SqlitePool) -> AppState {
        AppState {
            pool,
            base_url: None,
            secure_cookies: false,
        }
    }

    fn auth_jar(gid: &str, token: &str) -> CookieJar {
        CookieJar::new().add(Cookie::new(ids::cookie_name(gid), token.to_string()))
    }

    /// `(stored amount, sorted shares)` for the group's one live expense.
    async fn persisted(pool: &SqlitePool, gid: &str) -> (i64, Vec<(i64, i64)>) {
        let amount: i64 = sqlx::query_scalar(
            "SELECT amount FROM expenses WHERE group_id = ? AND deleted_at IS NULL",
        )
        .bind(gid)
        .fetch_one(pool)
        .await
        .unwrap();
        let mut shares = db::expense_share_rows(pool, gid).await.unwrap();
        shares.sort();
        (amount, shares)
    }

    async fn expense_count(pool: &SqlitePool, gid: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM expenses WHERE group_id = ?")
            .bind(gid)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn equal_split_persists_equal_shares_and_matching_total() {
        let pool = db::memory_pool().await;
        let (gid, [alice, bob, cara], token) = group_with_three(&pool).await;

        // 100.00 split equally among all three.
        let body = format!(
            "payer_id={alice}&description=Dinner&method=equal&amount=100&inc_{alice}=on&inc_{bob}=on&inc_{cara}=on"
        );
        let _ = add_expense(
            State(state(pool.clone())),
            Path(gid.clone()),
            auth_jar(&gid, &token),
            body,
        )
        .await
        .map_err(|_| "handler returned an error")
        .unwrap();

        // 10000 öre / 3 = 3334, 3333, 3333 — the leftover öre goes to the lowest id.
        let (amount, shares) = persisted(&pool, &gid).await;
        assert_eq!(shares, vec![(alice, 3334), (bob, 3333), (cara, 3333)]);
        assert_eq!(amount, 10000);
        assert_eq!(
            amount,
            shares.iter().map(|(_, a)| a).sum::<i64>(),
            "stored total must equal Σ shares"
        );
    }

    #[tokio::test]
    async fn exact_split_persists_submitted_amounts_and_conserves_money() {
        let pool = db::memory_pool().await;
        let (gid, [alice, bob, cara], token) = group_with_three(&pool).await;

        // Alice fronts it; Bob owes 80.00, Cara owes 40.50; Alice isn't splitting.
        let body = format!(
            "payer_id={alice}&description=Nachos&method=exact&amt_{bob}=80&amt_{cara}=40.50"
        );
        let _ = add_expense(
            State(state(pool.clone())),
            Path(gid.clone()),
            auth_jar(&gid, &token),
            body,
        )
        .await
        .map_err(|_| "handler returned an error")
        .unwrap();

        let (amount, shares) = persisted(&pool, &gid).await;
        assert_eq!(shares, vec![(bob, 8000), (cara, 4050)]);
        assert_eq!(
            amount, 12050,
            "exact total is the sum of the entered amounts"
        );

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
        let _ = send(format!(
            "payer_id={alice}&description=&method=equal&amount=50&inc_{alice}=on"
        ))
        .await;
        // Zero total.
        let _ = send(format!(
            "payer_id={alice}&description=X&method=equal&amount=0&inc_{alice}=on"
        ))
        .await;
        // No participants selected.
        let _ = send(format!(
            "payer_id={alice}&description=X&method=equal&amount=50"
        ))
        .await;
        // Exact split with only a non-positive amount.
        let _ = send(format!(
            "payer_id={alice}&description=X&method=exact&amt_{bob}=0"
        ))
        .await;
        // Payer isn't a member of the group.
        let _ = send(format!(
            "payer_id=999999&description=X&method=equal&amount=50&inc_{alice}=on"
        ))
        .await;

        assert_eq!(
            expense_count(&pool, &gid).await,
            0,
            "no invalid submission should persist"
        );
    }

    #[tokio::test]
    async fn unauthenticated_is_rejected_and_persists_nothing() {
        let pool = db::memory_pool().await;
        let (gid, [alice, _bob, _cara], _token) = group_with_three(&pool).await;
        let body = format!("payer_id={alice}&description=X&method=equal&amount=50&inc_{alice}=on");
        // No device cookie → Forbidden.
        let res = add_expense(
            State(state(pool.clone())),
            Path(gid.clone()),
            CookieJar::new(),
            body,
        )
        .await;
        assert!(
            res.is_err(),
            "a request with no device cookie must be rejected"
        );
        assert_eq!(expense_count(&pool, &gid).await, 0);
    }

    #[tokio::test]
    async fn closed_group_rejects_new_expenses() {
        let pool = db::memory_pool().await;
        let (gid, [alice, bob, cara], token) = group_with_three(&pool).await;
        sqlx::query("UPDATE groups SET closed_at = datetime('now') WHERE id = ?")
            .bind(&gid)
            .execute(&pool)
            .await
            .unwrap();

        let body = format!(
            "payer_id={alice}&description=Late&method=equal&amount=90&inc_{alice}=on&inc_{bob}=on&inc_{cara}=on"
        );
        let _ = add_expense(
            State(state(pool.clone())),
            Path(gid.clone()),
            auth_jar(&gid, &token),
            body,
        )
        .await;
        assert_eq!(
            expense_count(&pool, &gid).await,
            0,
            "a closed group must not accept new expenses"
        );
    }

    // --- Add-expense screen (GET /g/{id}/add) -------------------------------------

    #[tokio::test]
    async fn add_expense_page_renders_the_form_for_a_member() {
        let pool = db::memory_pool().await;
        let (gid, [_alice, bob, cara], token) = group_with_three(&pool).await;
        let resp = add_expense_page(
            State(state(pool.clone())),
            Path(gid.clone()),
            auth_jar(&gid, &token),
        )
        .await
        .map_err(|_| "member should see the form")
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let out = body_string(resp).await;
        assert!(
            out.contains("New expense"),
            "the focused screen has its heading"
        );
        assert!(
            out.contains(&format!(r#"action="/g/{gid}/expenses""#)),
            "posts to the expense endpoint"
        );
        // Every member is a selectable participant on this screen.
        assert!(
            out.contains(&format!(r#"name="inc_{bob}""#))
                && out.contains(&format!(r#"name="inc_{cara}""#))
        );
    }

    #[tokio::test]
    async fn add_expense_page_forbidden_for_non_member() {
        let pool = db::memory_pool().await;
        let (gid, _ids, _token) = group_with_three(&pool).await;
        let res = add_expense_page(
            State(state(pool.clone())),
            Path(gid.clone()),
            CookieJar::new(),
        )
        .await;
        assert!(
            res.is_err(),
            "a non-member must not reach the add-expense screen"
        );
    }

    #[tokio::test]
    async fn add_expense_page_redirects_when_closed() {
        let pool = db::memory_pool().await;
        let (gid, _ids, token) = group_with_three(&pool).await;
        sqlx::query("UPDATE groups SET closed_at = datetime('now') WHERE id = ?")
            .bind(&gid)
            .execute(&pool)
            .await
            .unwrap();
        let resp = add_expense_page(
            State(state(pool.clone())),
            Path(gid.clone()),
            auth_jar(&gid, &token),
        )
        .await
        .map_err(|_| "closed group should redirect, not error")
        .unwrap();
        assert!(
            resp.status().is_redirection(),
            "a closed group bounces back to the group page"
        );
        assert_eq!(
            resp.headers().get("location").unwrap(),
            &format!("/g/{gid}")
        );
    }

    // --- Edit expense (GET/POST /g/{id}/expenses/{eid}/edit) ----------------------

    /// Insert an extra member with a known raw device token; returns their id.
    async fn add_member(pool: &SqlitePool, gid: &str, name: &str, token: &str) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO members (group_id, name, token_hash) VALUES (?, ?, ?) RETURNING id",
        )
        .bind(gid)
        .bind(name)
        .bind(ids::hash_token(token))
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// The id of the group's single live expense.
    async fn only_expense_id(pool: &SqlitePool, gid: &str) -> i64 {
        sqlx::query_scalar("SELECT id FROM expenses WHERE group_id = ? AND deleted_at IS NULL")
            .bind(gid)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// Add one expense (as Alice) via the real handler and return its id.
    async fn seed(pool: &SqlitePool, gid: &str, token: &str, body: String) -> i64 {
        let _ = add_expense(
            State(state(pool.clone())),
            Path(gid.to_string()),
            auth_jar(gid, token),
            body,
        )
        .await
        .map_err(|_| "seeding an expense should not error")
        .unwrap();
        only_expense_id(pool, gid).await
    }

    async fn do_edit(pool: &SqlitePool, gid: &str, eid: i64, token: &str, body: String) {
        let _ = edit_expense(
            State(state(pool.clone())),
            Path((gid.to_string(), eid)),
            auth_jar(gid, token),
            body,
        )
        .await
        .map_err(|_| "edit should not error")
        .unwrap();
    }

    #[tokio::test]
    async fn edit_rewrites_shares_and_total_in_place() {
        let pool = db::memory_pool().await;
        let (gid, [alice, bob, cara], token) = group_with_three(&pool).await;
        // Seed: Alice pays 90.00 split equally three ways.
        let eid = seed(
            &pool,
            &gid,
            &token,
            format!(
                "payer_id={alice}&description=Dinner&method=equal&amount=90&inc_{alice}=on&inc_{bob}=on&inc_{cara}=on"
            ),
        )
        .await;

        // Edit → exact amounts: Bob owes 60.00, Cara owes 30.00, Alice out.
        do_edit(
            &pool,
            &gid,
            eid,
            &token,
            format!(
                "payer_id={alice}&description=Dinner+fixed&method=exact&amt_{bob}=60&amt_{cara}=30"
            ),
        )
        .await;

        let (amount, shares) = persisted(&pool, &gid).await;
        assert_eq!(shares, vec![(bob, 6000), (cara, 3000)]);
        assert_eq!(amount, 9000, "stored total tracks the re-split shares");
        assert_eq!(
            expense_count(&pool, &gid).await,
            1,
            "an edit updates in place — no delete-and-re-add"
        );
        let desc: String = sqlx::query_scalar("SELECT description FROM expenses WHERE id = ?")
            .bind(eid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(desc, "Dinner fixed");
    }

    #[tokio::test]
    async fn edit_folds_in_a_member_who_was_not_in_the_original_split() {
        // The headline "rebalance when new people join" case: a round logged before
        // someone was part of it, re-split to include them via an edit.
        let pool = db::memory_pool().await;
        let (gid, [alice, bob, cara], token) = group_with_three(&pool).await;
        // Alice pays 100.00, split equally between Alice and Bob only.
        let eid = seed(
            &pool,
            &gid,
            &token,
            format!("payer_id={alice}&description=Round&method=equal&amount=100&inc_{alice}=on&inc_{bob}=on"),
        )
        .await;
        assert_eq!(
            persisted(&pool, &gid).await.1,
            vec![(alice, 5000), (bob, 5000)]
        );

        // Cara is pulled into the round: re-split equally three ways.
        do_edit(
            &pool,
            &gid,
            eid,
            &token,
            format!(
                "payer_id={alice}&description=Round&method=equal&amount=100&inc_{alice}=on&inc_{bob}=on&inc_{cara}=on"
            ),
        )
        .await;

        let (amount, shares) = persisted(&pool, &gid).await;
        assert_eq!(shares, vec![(alice, 3334), (bob, 3333), (cara, 3333)]);
        assert_eq!(amount, 10000);
    }

    #[tokio::test]
    async fn owner_may_edit_another_members_expense() {
        let pool = db::memory_pool().await;
        let (gid, [alice, bob, cara], token) = group_with_three(&pool).await;
        // Expense paid by Bob (Alice, the owner, records it).
        let eid = seed(
            &pool,
            &gid,
            &token,
            format!(
                "payer_id={bob}&description=Cab&method=equal&amount=60&inc_{bob}=on&inc_{cara}=on"
            ),
        )
        .await;
        // Alice (owner, not the payer) re-splits it across everyone.
        do_edit(
            &pool,
            &gid,
            eid,
            &token,
            format!(
                "payer_id={bob}&description=Cab&method=equal&amount=60&inc_{alice}=on&inc_{bob}=on&inc_{cara}=on"
            ),
        )
        .await;
        let (amount, shares) = persisted(&pool, &gid).await;
        assert_eq!(amount, 6000);
        assert_eq!(
            shares.len(),
            3,
            "the owner re-split across the whole roster"
        );
    }

    #[tokio::test]
    async fn edit_forbidden_for_non_payer_non_owner() {
        let pool = db::memory_pool().await;
        let (gid, [alice, bob, cara], token) = group_with_three(&pool).await;
        add_member(&pool, &gid, "Dave", "dave-token").await;
        let eid = seed(
            &pool,
            &gid,
            &token,
            format!(
                "payer_id={alice}&description=Dinner&method=equal&amount=90&inc_{alice}=on&inc_{bob}=on&inc_{cara}=on"
            ),
        )
        .await;
        // Dave is neither the payer nor the owner.
        let res = edit_expense(
            State(state(pool.clone())),
            Path((gid.clone(), eid)),
            auth_jar(&gid, "dave-token"),
            format!("payer_id={alice}&description=Hacked&method=exact&amt_{alice}=1"),
        )
        .await;
        assert!(res.is_err(), "only the payer or owner may edit");
        assert_eq!(
            persisted(&pool, &gid).await.0,
            9000,
            "a rejected edit changes nothing"
        );
    }

    #[tokio::test]
    async fn edit_on_closed_group_persists_nothing() {
        let pool = db::memory_pool().await;
        let (gid, [alice, bob, cara], token) = group_with_three(&pool).await;
        let eid = seed(
            &pool,
            &gid,
            &token,
            format!(
                "payer_id={alice}&description=Dinner&method=equal&amount=90&inc_{alice}=on&inc_{bob}=on&inc_{cara}=on"
            ),
        )
        .await;
        sqlx::query("UPDATE groups SET closed_at = datetime('now') WHERE id = ?")
            .bind(&gid)
            .execute(&pool)
            .await
            .unwrap();
        // Handler returns a plain redirect but must not mutate.
        do_edit(
            &pool,
            &gid,
            eid,
            &token,
            format!("payer_id={alice}&description=Changed&method=exact&amt_{bob}=1"),
        )
        .await;
        assert_eq!(
            persisted(&pool, &gid).await.0,
            9000,
            "a closed group must not accept edits"
        );
    }

    #[tokio::test]
    async fn edit_page_prefills_the_stored_expense() {
        let pool = db::memory_pool().await;
        let (gid, [alice, bob, cara], token) = group_with_three(&pool).await;
        // Exact split: Bob 80.00, Cara 40.50, Alice out.
        let eid = seed(
            &pool,
            &gid,
            &token,
            format!(
                "payer_id={alice}&description=Nachos&method=exact&amt_{bob}=80&amt_{cara}=40.50"
            ),
        )
        .await;
        let resp = edit_expense_page(
            State(state(pool.clone())),
            Path((gid.clone(), eid)),
            auth_jar(&gid, &token),
        )
        .await
        .map_err(|_| "the payer should see the edit form")
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let out = body_string(resp).await;
        assert!(out.contains("Edit expense"), "has the edit heading");
        assert!(
            out.contains(&format!(r#"action="/g/{gid}/expenses/{eid}/edit""#)),
            "posts back to the edit endpoint"
        );
        assert!(
            out.contains(r#"value="Nachos""#),
            "description is prefilled"
        );
        assert!(
            out.contains(r#"value="80.00""#) && out.contains(r#"value="40.50""#),
            "each stored share amount is prefilled"
        );
    }

    #[tokio::test]
    async fn edit_page_forbidden_for_non_manager() {
        let pool = db::memory_pool().await;
        let (gid, [alice, bob, cara], token) = group_with_three(&pool).await;
        add_member(&pool, &gid, "Dave", "dave-token").await;
        let eid = seed(
            &pool,
            &gid,
            &token,
            format!(
                "payer_id={alice}&description=Dinner&method=equal&amount=90&inc_{alice}=on&inc_{bob}=on&inc_{cara}=on"
            ),
        )
        .await;
        let res = edit_expense_page(
            State(state(pool.clone())),
            Path((gid.clone(), eid)),
            auth_jar(&gid, "dave-token"),
        )
        .await;
        assert!(
            res.is_err(),
            "a non-payer non-owner cannot open the edit form"
        );
    }

    // --- Group create / join ------------------------------------------------------

    #[tokio::test]
    async fn create_group_persists_group_and_owner_and_sets_cookie() {
        let pool = db::memory_pool().await;
        let form = CreateForm {
            name: "  Trip  ".into(),
            your_name: "Alice".into(),
            currency: Some("eur".into()),
        };
        let (jar, redirect) = create_group(
            State(state(pool.clone())),
            CookieJar::new(),
            axum::Form(form),
        )
        .await
        .map_err(|_| "create should not error")
        .unwrap();

        let (gid, name, currency): (String, String, String) =
            sqlx::query_as("SELECT id, name, currency FROM groups")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            (name.as_str(), currency.as_str()),
            ("Trip", "EUR"),
            "name trimmed, currency uppercased"
        );
        let (owner_name, is_owner): (String, i64) =
            sqlx::query_as("SELECT name, is_owner FROM members WHERE group_id = ?")
                .bind(&gid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!((owner_name.as_str(), is_owner), ("Alice", 1));
        assert!(
            jar.get(&ids::cookie_name(&gid)).is_some(),
            "the owner's device cookie is set"
        );
        assert_eq!(
            redirect.into_response().headers().get("location").unwrap(),
            &format!("/g/{gid}")
        );
    }

    #[tokio::test]
    async fn join_group_adds_member_and_sets_cookie() {
        let pool = db::memory_pool().await;
        let (gid, _ids, _token) = group_with_three(&pool).await;
        let (jar, _r) = join_group(
            State(state(pool.clone())),
            Path(gid.clone()),
            CookieJar::new(),
            axum::Form(JoinForm {
                name: "Dave".into(),
            }),
        )
        .await
        .map_err(|_| "join should not error")
        .unwrap();
        let dave: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM members WHERE group_id = ? AND name = 'Dave' AND is_owner = 0",
        )
        .bind(&gid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(dave, 1, "a new non-owner member is inserted");
        assert!(
            jar.get(&ids::cookie_name(&gid)).is_some(),
            "the joiner's device cookie is set"
        );
    }

    // --- Delete expense -----------------------------------------------------------

    #[tokio::test]
    async fn delete_expense_soft_deletes_for_payer() {
        let pool = db::memory_pool().await;
        let (gid, [alice, bob, cara], token) = group_with_three(&pool).await;
        let eid = seed(
            &pool,
            &gid,
            &token,
            format!(
                "payer_id={alice}&description=Dinner&method=equal&amount=90&inc_{alice}=on&inc_{bob}=on&inc_{cara}=on"
            ),
        )
        .await;
        let _ = delete_expense(
            State(state(pool.clone())),
            Path((gid.clone(), eid)),
            auth_jar(&gid, &token),
        )
        .await
        .map_err(|_| "delete should not error")
        .unwrap();
        assert_eq!(
            expense_count(&pool, &gid).await,
            1,
            "the row remains — delete is a soft delete"
        );
        let live: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM expenses WHERE group_id = ? AND deleted_at IS NULL",
        )
        .bind(&gid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(live, 0, "but it no longer counts as live");
    }

    #[tokio::test]
    async fn delete_expense_forbidden_for_non_payer_non_owner() {
        let pool = db::memory_pool().await;
        let (gid, [alice, bob, cara], token) = group_with_three(&pool).await;
        add_member(&pool, &gid, "Dave", "dave-token").await;
        let eid = seed(
            &pool,
            &gid,
            &token,
            format!(
                "payer_id={alice}&description=Dinner&method=equal&amount=90&inc_{alice}=on&inc_{bob}=on&inc_{cara}=on"
            ),
        )
        .await;
        let res = delete_expense(
            State(state(pool.clone())),
            Path((gid.clone(), eid)),
            auth_jar(&gid, "dave-token"),
        )
        .await;
        assert!(res.is_err(), "only the payer or owner may delete");
        let live: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM expenses WHERE group_id = ? AND deleted_at IS NULL",
        )
        .bind(&gid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(live, 1, "a rejected delete changes nothing");
    }

    // --- Settlements --------------------------------------------------------------

    /// The single settlement amount recorded for a group.
    async fn only_settlement_amount(pool: &SqlitePool, gid: &str) -> Option<i64> {
        sqlx::query_scalar("SELECT amount FROM settlements WHERE group_id = ?")
            .bind(gid)
            .fetch_optional(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn mark_settlement_clamps_to_outstanding_debt() {
        let pool = db::memory_pool().await;
        let (gid, [alice, bob, cara], token) = group_with_three(&pool).await;
        // Alice pays 90.00 split three ways → Bob owes her 30.00.
        let _ = seed(
            &pool,
            &gid,
            &token,
            format!(
                "payer_id={alice}&description=Dinner&method=equal&amount=90&inc_{alice}=on&inc_{bob}=on&inc_{cara}=on"
            ),
        )
        .await;
        // Bob offers 100.00; it must clamp to the 30.00 he actually owes.
        let _ = mark_settlement(
            State(state(pool.clone())),
            Path(gid.clone()),
            auth_jar(&gid, &token),
            axum::Form(SettlementForm {
                from_id: bob,
                to_id: alice,
                amount_ore: 10000,
            }),
        )
        .await
        .map_err(|_| "settlement should not error")
        .unwrap();
        assert_eq!(
            only_settlement_amount(&pool, &gid).await,
            Some(3000),
            "clamped to Bob's 30.00 debt"
        );
    }

    #[tokio::test]
    async fn mark_settlement_rejects_self_and_nonmembers() {
        let pool = db::memory_pool().await;
        let (gid, [alice, bob, _cara], token) = group_with_three(&pool).await;
        let _ = seed(
            &pool,
            &gid,
            &token,
            format!(
                "payer_id={alice}&description=Dinner&method=equal&amount=90&inc_{alice}=on&inc_{bob}=on"
            ),
        )
        .await;
        // Same person on both sides, and an id that isn't in the group.
        for (from_id, to_id) in [(alice, alice), (bob, 999_999)] {
            let _ = mark_settlement(
                State(state(pool.clone())),
                Path(gid.clone()),
                auth_jar(&gid, &token),
                axum::Form(SettlementForm {
                    from_id,
                    to_id,
                    amount_ore: 1000,
                }),
            )
            .await
            .unwrap();
        }
        assert_eq!(
            only_settlement_amount(&pool, &gid).await,
            None,
            "neither a self-transfer nor a non-member settles"
        );
    }

    // --- Close / reopen / recovery ------------------------------------------------

    async fn is_closed(pool: &SqlitePool, gid: &str) -> bool {
        db::load_group(pool, gid)
            .await
            .unwrap()
            .unwrap()
            .is_closed()
    }

    #[tokio::test]
    async fn close_then_reopen_toggles_closed_state() {
        let pool = db::memory_pool().await;
        let (gid, _ids, token) = group_with_three(&pool).await;
        let _ = close_group(
            State(state(pool.clone())),
            Path(gid.clone()),
            auth_jar(&gid, &token),
        )
        .await
        .unwrap();
        assert!(is_closed(&pool, &gid).await, "closed after close");
        let _ = reopen_group(
            State(state(pool.clone())),
            Path(gid.clone()),
            auth_jar(&gid, &token),
        )
        .await
        .unwrap();
        assert!(!is_closed(&pool, &gid).await, "open again after reopen");
    }

    #[tokio::test]
    async fn close_group_forbidden_for_non_owner() {
        let pool = db::memory_pool().await;
        let (gid, _ids, _token) = group_with_three(&pool).await;
        add_member(&pool, &gid, "Dave", "dave-token").await;
        let res = close_group(
            State(state(pool.clone())),
            Path(gid.clone()),
            auth_jar(&gid, "dave-token"),
        )
        .await;
        assert!(res.is_err(), "only the owner may close");
        assert!(!is_closed(&pool, &gid).await, "the group stays open");
    }

    async fn last_active(pool: &SqlitePool, gid: &str) -> String {
        sqlx::query_scalar("SELECT last_active FROM groups WHERE id = ?")
            .bind(gid)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// Push `last_active` a day into the past so a subsequent bump (or its absence) is
    /// unambiguous — avoids comparing two same-second `datetime('now')` values.
    async fn backdate(pool: &SqlitePool, gid: &str) {
        sqlx::query("UPDATE groups SET last_active = datetime('now', '-1 day') WHERE id = ?")
            .bind(gid)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn close_group_does_not_bump_last_active() {
        // Closing an unclaimed group must NOT reset its expiry clock: a wound-down
        // throwaway should still expire ~INACTIVE_DAYS after its last *real* activity,
        // not from the moment it was closed.
        let pool = db::memory_pool().await;
        let (gid, _ids, token) = group_with_three(&pool).await;
        backdate(&pool, &gid).await;
        let before = last_active(&pool, &gid).await;
        let _ = close_group(
            State(state(pool.clone())),
            Path(gid.clone()),
            auth_jar(&gid, &token),
        )
        .await
        .unwrap();
        assert_eq!(
            last_active(&pool, &gid).await,
            before,
            "close leaves last_active untouched"
        );
    }

    #[tokio::test]
    async fn delete_expense_bumps_last_active() {
        let pool = db::memory_pool().await;
        let (gid, [alice, bob, cara], token) = group_with_three(&pool).await;
        let eid = seed(
            &pool,
            &gid,
            &token,
            format!(
                "payer_id={alice}&description=Dinner&method=equal&amount=90&inc_{alice}=on&inc_{bob}=on&inc_{cara}=on"
            ),
        )
        .await;
        backdate(&pool, &gid).await;
        let before = last_active(&pool, &gid).await;
        let _ = delete_expense(
            State(state(pool.clone())),
            Path((gid.clone(), eid)),
            auth_jar(&gid, &token),
        )
        .await
        .unwrap();
        assert_ne!(
            last_active(&pool, &gid).await,
            before,
            "a delete counts as activity and bumps last_active"
        );
    }

    async fn owner_token_hash(pool: &SqlitePool, gid: &str) -> String {
        sqlx::query_scalar("SELECT token_hash FROM members WHERE group_id = ? AND is_owner = 1")
            .bind(gid)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn set_recovery_stores_trimmed_hashed_passphrase() {
        let pool = db::memory_pool().await;
        let (gid, _ids, token) = group_with_three(&pool).await;
        let _ = set_recovery(
            State(state(pool.clone())),
            Path(gid.clone()),
            auth_jar(&gid, &token),
            axum::Form(RecoveryForm {
                passphrase: "  hunter2 ".into(),
            }),
        )
        .await
        .unwrap();
        let stored: Option<String> = sqlx::query_scalar("SELECT recovery FROM groups WHERE id = ?")
            .bind(&gid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            stored,
            Some(ids::hash_token("hunter2")),
            "passphrase is trimmed then hashed"
        );
    }

    #[tokio::test]
    async fn recover_submit_rotates_owner_token_on_correct_passphrase() {
        let pool = db::memory_pool().await;
        let (gid, _ids, token) = group_with_three(&pool).await;
        let _ = set_recovery(
            State(state(pool.clone())),
            Path(gid.clone()),
            auth_jar(&gid, &token),
            axum::Form(RecoveryForm {
                passphrase: "hunter2".into(),
            }),
        )
        .await
        .unwrap();
        let old_hash = owner_token_hash(&pool, &gid).await;

        // Correct passphrase from a fresh device → redirect in and rotate the token.
        let resp = recover_submit(
            State(state(pool.clone())),
            Path(gid.clone()),
            CookieJar::new(),
            axum::Form(RecoveryForm {
                passphrase: "hunter2".into(),
            }),
        )
        .await
        .unwrap();
        assert!(
            resp.status().is_redirection(),
            "correct passphrase lets you in"
        );
        assert_ne!(
            owner_token_hash(&pool, &gid).await,
            old_hash,
            "the owner's device token is rotated onto the new device"
        );
    }

    #[tokio::test]
    async fn recover_submit_rejects_wrong_passphrase() {
        let pool = db::memory_pool().await;
        let (gid, _ids, token) = group_with_three(&pool).await;
        let _ = set_recovery(
            State(state(pool.clone())),
            Path(gid.clone()),
            auth_jar(&gid, &token),
            axum::Form(RecoveryForm {
                passphrase: "hunter2".into(),
            }),
        )
        .await
        .unwrap();
        let old_hash = owner_token_hash(&pool, &gid).await;
        let resp = recover_submit(
            State(state(pool.clone())),
            Path(gid.clone()),
            CookieJar::new(),
            axum::Form(RecoveryForm {
                passphrase: "wrong".into(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "a wrong passphrase just re-renders the form"
        );
        assert_eq!(
            owner_token_hash(&pool, &gid).await,
            old_hash,
            "and rotates nothing"
        );
    }

    // --- Live-update poll ---------------------------------------------------------

    async fn body_string(resp: Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
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
        assert_eq!(
            resp.status(),
            StatusCode::NO_CONTENT,
            "no change → swap nothing"
        );
    }

    #[tokio::test]
    async fn live_expense_change_sends_oob_fragments_without_notice() {
        let pool = db::memory_pool().await;
        let (gid, [alice, bob, cara], token) = group_with_three(&pool).await;
        let body = format!(
            "payer_id={alice}&description=Dinner&method=equal&amount=100&inc_{alice}=on&inc_{bob}=on&inc_{cara}=on"
        );
        let _ = add_expense(
            State(state(pool.clone())),
            Path(gid.clone()),
            auth_jar(&gid, &token),
            body,
        )
        .await
        .map_err(|_| "add failed")
        .unwrap();

        // Client saw an older token but the same 3-member, open roster.
        let resp = poll(&pool, &gid, &token, q(Some(0), Some(3), Some(0))).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let out = body_string(resp).await;
        assert!(out.contains("hx-swap-oob"), "must carry out-of-band swaps");
        assert!(
            out.contains(r#"id="ls-ledger""#),
            "read-only ledger region is swapped"
        );
        assert!(
            out.contains("Dinner"),
            "the new expense appears in the swapped ledger"
        );
        assert!(
            out.contains(r#"id="ls-poll""#),
            "poller token is re-sent so the guard advances"
        );
        assert!(
            !out.contains("Someone just joined"),
            "an expense is not a join"
        );
        assert!(
            !out.contains(r#"name="amount""#),
            "the add-expense form is never sent"
        );
    }

    #[tokio::test]
    async fn live_join_includes_the_notice() {
        let pool = db::memory_pool().await;
        let (gid, _ids, token) = group_with_three(&pool).await;
        // A fourth person joins.
        sqlx::query("INSERT INTO members (group_id, name, token_hash) VALUES (?, 'Dave', 'hd')")
            .bind(&gid)
            .execute(&pool)
            .await
            .unwrap();
        db::touch_group(&pool, &gid).await.unwrap();

        // Client still thinks there are 3 members.
        let resp = poll(&pool, &gid, &token, q(Some(0), Some(3), Some(0))).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let out = body_string(resp).await;
        assert!(
            out.contains("Someone just joined"),
            "a roster growth shows the join notice"
        );
        assert!(out.contains("Refresh to include them"));
    }

    #[tokio::test]
    async fn live_close_and_reopen_trigger_full_reload() {
        let pool = db::memory_pool().await;
        let (gid, _ids, token) = group_with_three(&pool).await;

        // Owner closes; a viewer who last saw it open must be told to reload.
        sqlx::query("UPDATE groups SET closed_at = datetime('now') WHERE id = ?")
            .bind(&gid)
            .execute(&pool)
            .await
            .unwrap();
        let resp = poll(&pool, &gid, &token, q(Some(0), Some(3), Some(0))).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("HX-Refresh").unwrap(),
            "true",
            "close → reload"
        );

        // And a viewer who last saw it closed must be told to reload on reopen.
        sqlx::query("UPDATE groups SET closed_at = NULL WHERE id = ?")
            .bind(&gid)
            .execute(&pool)
            .await
            .unwrap();
        let resp = poll(&pool, &gid, &token, q(Some(0), Some(3), Some(1))).await;
        assert_eq!(
            resp.headers().get("HX-Refresh").unwrap(),
            "true",
            "reopen → reload"
        );
    }

    #[tokio::test]
    async fn live_closed_and_current_does_not_loop_on_refresh() {
        // A closed group the client already knows is closed, with nothing new, must
        // return 204 — never a fresh HX-Refresh, or open pages would reload forever.
        let pool = db::memory_pool().await;
        let (gid, _ids, token) = group_with_three(&pool).await;
        sqlx::query("UPDATE groups SET closed_at = datetime('now') WHERE id = ?")
            .bind(&gid)
            .execute(&pool)
            .await
            .unwrap();
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
        assert!(
            res.is_err(),
            "a non-member must not receive group fragments"
        );
    }
}
