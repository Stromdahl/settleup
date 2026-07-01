//! HTML rendering with maud. Everything works as plain form POST + redirect; htmx
//! (loaded from CDN, with `hx-boost`) is a progressive enhancement, so the app is
//! fully functional even if the script never loads.

use crate::money::format_amount;
use maud::{DOCTYPE, Markup, PreEscaped, html};

const STYLES: &str = r#"
:root { --bg:#0f1115; --card:#1a1d24; --line:#2a2f3a; --fg:#e7e9ee;
        --muted:#9aa3b2; --accent:#4f8cff; --pos:#4ade80; --neg:#f87171; }
* { box-sizing: border-box; }
body { margin:0; font:16px/1.5 system-ui, sans-serif; background:var(--bg);
       color:var(--fg); }
.wrap { max-width:640px; margin:0 auto; padding:20px 16px 80px; }
h1 { font-size:1.5rem; margin:.2em 0; }
h2 { font-size:1.05rem; color:var(--muted); text-transform:uppercase;
     letter-spacing:.04em; margin:1.6em 0 .6em; }
a { color:var(--accent); }
.card { background:var(--card); border:1px solid var(--line); border-radius:12px;
        padding:14px 16px; margin:10px 0; }
.row { display:flex; justify-content:space-between; align-items:center; gap:10px; }
.muted { color:var(--muted); font-size:.9rem; }
.pos { color:var(--pos); } .neg { color:var(--neg); }
label { display:block; margin:10px 0 4px; font-size:.9rem; color:var(--muted); }
input[type=text], input[type=number], input[type=password], select {
    width:100%; padding:10px; border-radius:8px; border:1px solid var(--line);
    background:#0d0f14; color:var(--fg); font-size:1rem; }
.inline { display:flex; gap:8px; align-items:center; }
.inline input { width:auto; }
button, .btn { display:inline-block; background:var(--accent); color:#fff; border:0;
    padding:10px 16px; border-radius:8px; font-size:1rem; cursor:pointer;
    text-decoration:none; }
button.secondary, .btn.secondary { background:#2a2f3a; }
button.danger { background:transparent; color:var(--neg); padding:4px 8px;
    font-size:.85rem; }
.split-list { margin:6px 0; }
.split-list .line { display:flex; align-items:center; gap:8px; padding:4px 0;
    border-bottom:1px solid var(--line); }
.split-list .line input[type=number] { width:100px; }
.split-list .grow { flex:1; }
.badge { font-size:.75rem; background:#2a2f3a; color:var(--muted);
    padding:2px 8px; border-radius:999px; }
.qr { background:#fff; padding:10px; border-radius:10px; display:inline-block; }
.qr svg { display:block; width:180px; height:180px; }
form.inlineform { display:inline; }
hr { border:0; border-top:1px solid var(--line); margin:20px 0; }
.pay-btn { white-space:nowrap; }
"#;

fn layout(title: &str, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) }
                style { (PreEscaped(STYLES)) }
                script src="https://unpkg.com/htmx.org@2.0.4" defer {}
            }
            body hx-boost="true" {
                div.wrap { (body) }
            }
        }
    }
}

/// Landing page: create a new group.
pub fn landing() -> Markup {
    layout(
        "SettleUp",
        html! {
            h1 { "SettleUp" }
            p.muted { "Split a bar tab or your monthly expenses. Create a group, share the link, everyone adds what they paid, then settle up." }
            form method="post" action="/" .card {
                label for="name" { "Group name" }
                input type="text" name="name" id="name" placeholder="Friday drinks" required;
                label for="your_name" { "Your name" }
                input type="text" name="your_name" id="your_name" placeholder="You" required;
                label for="currency" { "Currency" }
                input type="text" name="currency" id="currency" value="SEK" maxlength="3";
                p {}
                button type="submit" { "Create group" }
            }
        },
    )
}

/// Shown when a visitor lands on a group link but isn't a member yet.
pub fn claim(group: &crate::models::Group) -> Markup {
    layout(
        &format!("Join {}", group.name),
        html! {
            h1 { "Join “" (group.name) "”" }
            p.muted { "Enter your name to join this group." }
            form method="post" action={ "/g/" (group.id) "/join" } .card {
                label for="name" { "Your name" }
                input type="text" name="name" id="name" placeholder="Your name" required;
                p {}
                button type="submit" { "Join" }
            }
            @if group.has_recovery() {
                p.muted { a href={ "/g/" (group.id) "/recover" } { "Owner? Recover access" } }
            }
        },
    )
}

/// Recovery page: enter the passphrase to re-claim owner access on a new device.
pub fn recover(group: &crate::models::Group, error: bool) -> Markup {
    layout(
        &format!("Recover {}", group.name),
        html! {
            h1 { "Recover “" (group.name) "”" }
            p.muted { "Enter the recovery passphrase set for this group to restore owner access on this device." }
            @if error {
                p.neg { "That passphrase didn't match." }
            }
            form method="post" action={ "/g/" (group.id) "/recover" } .card {
                label for="passphrase" { "Recovery passphrase" }
                input type="password" name="passphrase" id="passphrase" required;
                p {}
                button type="submit" { "Recover access" }
            }
        },
    )
}

pub struct MemberRow {
    pub id: i64,
    pub name: String,
    pub is_owner: bool,
}
pub struct BalanceRow {
    pub name: String,
    pub net: i64,
}
pub struct TransferRow {
    pub from_id: i64,
    pub from: String,
    pub to_id: i64,
    pub to: String,
    pub amount: i64,
}
pub struct ExpenseRow {
    pub id: i64,
    pub payer: String,
    pub amount: i64,
    pub description: String,
    pub participants: String,
    pub created_at: String,
    pub can_delete: bool,
}
pub struct SettlementRow {
    pub from: String,
    pub to: String,
    pub amount: i64,
    pub created_at: String,
}

pub struct GroupView<'a> {
    pub group: &'a crate::models::Group,
    pub me: &'a crate::models::Member,
    pub join_url: &'a str,
    pub members: Vec<MemberRow>,
    pub balances: Vec<BalanceRow>,
    pub transfers: Vec<TransferRow>,
    pub expenses: Vec<ExpenseRow>,
    pub settlements: Vec<SettlementRow>,
}

fn money(amount: i64, currency: &str) -> String {
    format!("{} {}", format_amount(amount), currency)
}

/// SQLite timestamps are `YYYY-MM-DD HH:MM:SS`; show minute precision.
fn short_dt(s: &str) -> &str {
    s.get(..16).unwrap_or(s)
}

/// The main group page.
pub fn group_page(v: &GroupView) -> Markup {
    let g = v.group;
    let cur = &g.currency;
    let closed = g.is_closed();
    layout(
        &g.name,
        html! {
            div.row {
                h1 { (g.name) }
                @if closed { span.badge { "closed" } }
            }
            p.muted {
                "You are " b { (v.me.name) }
                @if v.me.is_owner { " · owner" }
                " · " (v.members.len()) " member" @if v.members.len() != 1 { "s" }
            }

            // --- Members ---
            div.card {
                h2 { "Members" }
                @for m in &v.members {
                    div.row {
                        span { (m.name) }
                        @if m.is_owner { span.badge { "owner" } }
                    }
                }
            }

            // --- Share / join ---
            div.card {
                h2 { "Invite" }
                p.muted { "Share this link or QR so others can join:" }
                p { a href=(v.join_url) { (v.join_url) } }
                div.qr { (PreEscaped(qr_svg(v.join_url))) }
            }

            // --- Balances ---
            h2 { "Balances" }
            div.card {
                @if v.balances.iter().all(|b| b.net == 0) {
                    p.muted { "All settled up." }
                } @else {
                    @for b in &v.balances {
                        div.row {
                            span { (b.name) }
                            @if b.net > 0 {
                                span.pos { "is owed " (money(b.net, cur)) }
                            } @else if b.net < 0 {
                                span.neg { "owes " (money(-b.net, cur)) }
                            } @else {
                                span.muted { "settled" }
                            }
                        }
                    }
                }
            }

            // --- Suggested settle-up transfers ---
            @if !v.transfers.is_empty() && !closed {
                h2 { "Settle up" }
                div.card {
                    p.muted { "The simplest way to square everyone:" }
                    @for t in &v.transfers {
                        div.row {
                            span { (t.from) " → " (t.to) " " b { (money(t.amount, cur)) } }
                            form.inlineform method="post" action={ "/g/" (g.id) "/settlements" } {
                                input type="hidden" name="from_id" value=(t.from_id);
                                input type="hidden" name="to_id" value=(t.to_id);
                                input type="hidden" name="amount_ore" value=(t.amount);
                                button.pay-btn type="submit" { "Mark paid" }
                            }
                        }
                    }
                }
            }

            // --- Add expense ---
            @if !closed {
                h2 { "Add expense" }
                form method="post" action={ "/g/" (g.id) "/expenses" } .card {
                    label for="description" { "What for?" }
                    input type="text" name="description" id="description" placeholder="Dinner, taxi, groceries…" required;

                    label for="payer_id" { "Who paid?" }
                    select name="payer_id" id="payer_id" {
                        @for m in &v.members {
                            option value=(m.id) selected[m.id == v.me.id] { (m.name) }
                        }
                    }

                    label { "Split" }
                    div.inline {
                        label.inline style="margin:0" { input type="radio" name="method" value="equal" checked; " Equally" }
                        label.inline style="margin:0" { input type="radio" name="method" value="exact"; " Exact amounts" }
                    }

                    label for="amount" { "Total (for equal split)" }
                    input type="text" name="amount" id="amount" inputmode="decimal" placeholder="0.00";

                    label { "Who shares it?" }
                    p.muted style="margin-top:0" { "Equal: tick who's in. Exact: type each person's amount." }
                    div.split-list {
                        @for m in &v.members {
                            div.line {
                                label.inline style="margin:0" {
                                    input type="checkbox" name={ "inc_" (m.id) } value="1" checked;
                                }
                                span.grow { (m.name) }
                                input type="text" name={ "amt_" (m.id) } inputmode="decimal" placeholder="0.00";
                            }
                        }
                    }
                    p {}
                    button type="submit" { "Add expense" }
                }
            } @else {
                div.card {
                    p.muted { "This group is closed." }
                    @if v.me.is_owner {
                        form.inlineform method="post" action={ "/g/" (g.id) "/reopen" } {
                            button.secondary type="submit" { "Reopen group" }
                        }
                    }
                }
            }

            // --- Expense log ---
            h2 { "Expenses" }
            @if v.expenses.is_empty() {
                div.card { p.muted { "No expenses yet." } }
            } @else {
                @for e in &v.expenses {
                    div.card {
                        div.row {
                            span { b { (e.description) } }
                            span { (money(e.amount, cur)) }
                        }
                        div.row {
                            span.muted { (e.payer) " paid · split: " (e.participants) }
                            span.muted { (short_dt(&e.created_at)) }
                        }
                        div.row {
                            span {}
                            @if e.can_delete {
                                form.inlineform method="post" action={ "/g/" (g.id) "/expenses/" (e.id) "/delete" } {
                                    button.danger type="submit" { "delete" }
                                }
                            }
                        }
                    }
                }
            }

            // --- Settlement log ---
            @if !v.settlements.is_empty() {
                h2 { "Payments" }
                @for s in &v.settlements {
                    div.card {
                        div.row {
                            span { (s.from) " paid " (s.to) }
                            span.pos { (money(s.amount, cur)) }
                        }
                        div.row {
                            span.muted { (short_dt(&s.created_at)) }
                        }
                    }
                }
            }

            // --- Owner controls ---
            @if v.me.is_owner {
                hr;
                h2 { "Owner controls" }
                div.card {
                    @if !closed {
                        form.inlineform method="post" action={ "/g/" (g.id) "/close" } {
                            button.secondary type="submit" { "Settle & close" }
                        }
                        " "
                    }
                    @if !g.has_recovery() {
                        form method="post" action={ "/g/" (g.id) "/recovery" } style="margin-top:12px" {
                            label for="passphrase" { "Keep this group: set a recovery passphrase" }
                            p.muted style="margin-top:0" { "Without one, this group is auto-deleted after a few days of inactivity. With one, it's kept and you can restore access on another device." }
                            div.inline {
                                input type="password" name="passphrase" id="passphrase" placeholder="passphrase" required;
                                button type="submit" { "Save" }
                            }
                        }
                    } @else {
                        p.muted { "Recovery passphrase is set — this group is kept." }
                    }
                }
            }
        },
    )
}

/// Render a QR code for the given URL as an inline SVG string.
fn qr_svg(url: &str) -> String {
    use qrcode::QrCode;
    use qrcode::render::svg;
    match QrCode::new(url.as_bytes()) {
        Ok(code) => code
            .render::<svg::Color>()
            .min_dimensions(180, 180)
            .quiet_zone(false)
            .build(),
        Err(_) => String::new(),
    }
}
