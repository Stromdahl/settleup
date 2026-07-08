# SettleUp — Decisions

A running log of the durable design decisions for this project, with the reasoning
behind each so future-you doesn't re-litigate them. Newest decisions can be appended
at the bottom. Captured from the initial design grill on 2026-07-01.

## Product

**What it is:** a real, deployable web app for splitting shared expenses. Two target
use cases drive every decision:

- **Bar tab** — a few strangers, in person, split a tab, then abandon it within the hour.
- **Monthly split** — two known people (e.g. partners) accumulate shared expenses over a
  month and settle up recurringly, wanting the history to persist.

These pull in opposite directions on identity and lifetime; resolving that collision is
the core of the design below.

---

## 1. Real project, not a playground

**Decision:** Build for real deployment and use, not as a Rust learning exercise.

**Why:** The two use cases are concrete and lived. Optimize for the people using it —
above all "strangers can join in seconds with zero friction" — not for what's fun to
build. This single answer forces most of the architecture below.

## 2. Topology: hosted Rust backend, browser client

**Decision:** A Rust backend hosted on the public internet, reachable by URL. QR codes
encode a group URL. The client is a web page — nothing to install.

**Why:** "Five strangers join instantly" ⇒ no app install ⇒ browser client ⇒ a hosted,
reachable server. Bar wifi is hostile and participants aren't guaranteed on one network,
which rules out pure peer-to-peer and LAN-only designs.

## 3. Client shape: server-rendered HTML + htmx

**Decision:** `axum` backend serving server-rendered HTML (templating via `maud`) with
`htmx` for the interactive bits. No SPA, no JS build step.

**Why:** The interactions are forms and lists (add expense, see balances, settle). Server
rendering gives the fastest first paint for a stranger on cellular, the smallest deploy
surface, and keeps everything in one language. htmx covers the few spots that want a
live feel without adopting a whole SPA framework.

**Alternatives considered:** Rust WASM frontend (Leptos/Dioxus) — bigger toolchain and
download, rejected for v1; separate JS/TS SPA — two ecosystems, least "Rust project".

## 4. Identity: owner token + guest device tokens, no accounts

**Decision:** No user accounts. The group creator's browser stores a secret **owner
token** (the token *is* the key). Joiners **claim a name** and get their own **guest
device token**. An **optional recovery** (email or passphrase) can be set on a group the
owner wants to keep, as an escape hatch against a lost/switched device.

**Why:** Accounts-for-everyone kills the bar case (nobody signs up for a round of beers);
no-identity kills the monthly case (you want it to persist and be yours). Token-per-device
is the in-between. Setting recovery doubles as the signal "this group matters."

**Consequence:** Clearing browser data or switching phones loses the handle on a group
unless recovery was set or the link is re-shared. Acceptable for bar tabs; recovery covers
the persistent case.

## 5. One group object; lifetime is behavior, not type

**Decision:** A single concept — a **group** (shared ledger with members, expenses,
settlements). Bar tab vs. monthly is behavioral, not a different type. Groups are
**persistent by default**. A **"settle & close"** action archives a group and is
**reopenable**. **Auto-expiry** sweeps groups inactive for N days that were never claimed
with recovery info.

**Why:** Users can't sensibly pick "ephemeral vs persistent" up front, and it would double
the UI. Make persistence the default and let throwaway bar tabs clean themselves up; the
only "I care about this" signal is whether recovery was set.

## 6. Expenses: single payer, equal or exact split, subset of members

**Decision:** An expense = **single payer, total amount, description, the subset of
members it's split across, and a split method (equal or exact amounts)**. Anyone can
delete their own expenses; the owner can delete any. Deletes are **soft**; balances
recompute. In v1, editing is **delete-and-re-add** — an in-place edit form is a later
addition, not part of the first slice.

**Why:** Equal covers most of both cases; exact is the escape hatch; subset selection is
needed at a bar table where not everyone's in on every round. Itemized splitting and
percentages/shares are deliberately deferred — itemized especially is a rabbit hole that
risks never shipping. One payer per expense keeps the model simple; "we both paid" is
worked around with two expenses.

## 7. Settling up: simplified transfers + mark-as-settled

**Decision:** Compute net balances, then present the **fewest transfers** that square the
group (debt simplification). Each suggested payment is **recordable ("mark paid")**; a
settlement is just a special transaction with an audit trail.

**Why:** Raw pairwise debts at a 5-person table are an unusable snarl; "here are the 3
payments that settle everyone" is the value proposition. Mark-as-settled answers "did I
already pay her back for June?" for the persistent case.

**Consequence:** A simplified transfer can be between two people who never directly
transacted; occasionally surprising but worth it. A "show detail" view can come later.

**Measured:** The simplifier is a greedy largest-debtor-pays-largest-creditor pass; the
true fewest-transfers problem is NP-hard, so "fewest" is a near-minimum, not a proven
optimum. The randomized campaign (`src/sim.rs`, `cargo test --release sim_campaign`)
quantifies the gap: over ~72M random groups checked against an exact-optimum oracle,
greedy matched the true minimum **99.15%** of the time, and when it didn't the penalty
was **+1 transfer on average, +3 at most**. An adversarial search over ~270M balance
vectors could not push the gap past **+3** either — so +3 is effectively the worst case
at realistic group sizes. Good enough that a smarter (exponential) optimizer isn't worth
it. The campaign also confirmed money is conserved, balances sum to zero, and the
suggested transfers settle everyone exactly, across groups from 2 to 800 members.

## 8. Currency, history, notifications

**Decision:** **Single currency per group, default SEK**, no conversion. A **flat log** of
expenses + settlements; no monthly-period concept. **No notifications** in v1.

**Why:** Multi-currency drags in rate-locking neither use case needs. Monthly-period
reporting and notifications are whole subsystems that aren't required to ship the core.

## 9. Swish deferred, but designed for

**Decision:** No payment-provider integration in v1, but the **settlement concept is
designed so a Swish deep-link can hang off "mark paid" later**.

**Why:** Swish is a natural fit (Swedish context) and a nice v2 touch, but wiring payment
providers into v1 is scope the core cases don't need.

## 10. Live updates: 5-second polling with out-of-band swaps

**Decision:** Open group pages **live-update** so one person's action shows up on everyone
else's already-open screen. A hidden htmx poller hits a new read-only `GET /g/{id}/live`
endpoint **every 5 seconds**, carrying the `(last_active, member_count, closed?)` state it
last rendered. The server responds one of three ways:

- **`204 No Content`** when nothing changed — htmx swaps nothing. Idle groups cost one
  indexed read per viewer per tick and no render work.
- **Out-of-band (OOB) fragment swaps** when `last_active` moved (an expense or settlement
  on the existing roster) — surgically replaces the read-only regions (settle-up,
  balances, expense log, payments log, member count) by id, and **never sends the
  add-expense form**, so in-progress input is safe by construction.
- **OOB swaps + a dismissible "someone joined — refresh to split with them" notice** when
  `member_count` grew (a join). A join is ~90% read-only (header count, a net-zero balances
  row) and only its *tail* — the form's payer/participant selectors — is structural, so it
  is handled non-destructively as a data change plus a notice, **not** a reload: the
  newcomer appears in the read-only regions immediately and is one tap away in the form,
  while whatever the viewer is typing survives untouched. (Injecting the newcomer directly
  into the selectors via append-style OOB is a deferred v2 polish.)
- **`HX-Refresh: true`** (a clean full reload) only when `closed?` changed — a close or
  reopen, which restructures the page (the sticky FAB, the closed banner, the presence of
  the form itself). This is lossless in practice: a closed group renders no add-expense
  form, so a close-while-typing destroys only input that couldn't have been submitted, and
  reopen-while-typing can't happen because no form existed.

Every content-bearing response also **OOB-updates the poller's own token** (its `hx-get`
URL / `hx-vals`) to the new `(last_active, member_count, closed?)`, so the next tick
carries what was just rendered. Without this the server sees a stale token and mismatches
on every tick — the guard silently degrades to unconditional re-swapping for any group
that has ever changed.

**Why:** Polling — not SSE or WebSocket — is stateless: it works identically on one
instance or many replicas behind a load balancer, survives restarts and flaky mobile
connections for free, and adds no dependency or open-connection state. In-process SSE
would be instant and cheaper-when-idle, but silently breaks the moment a second replica
exists (a change on one replica never reaches viewers pinned to another) — the wrong bet
for a "prepare for scale" posture, and SQLite's single-writer, file-local nature means
genuine multi-replica is a Postgres-sized change away regardless. A bar tab tolerates ~5s
latency effortlessly. OOB swaps from a single endpoint keep the frequent case to one
request and surgical DOM updates while leaving the input form untouched; the `last_active`
version guard (a field every mutation already bumps via `touch_group`) stops idle viewers
from re-running the balance query and re-swapping five regions forever.

**Consequence:** Only close/reopen cost a full reload — accepted, because they are rare,
lossless (no submittable input exists across those transitions), and a reload is the
honest way to rebuild the page structure. A join preserves in-progress input at the price
that the newcomer isn't selectable in the form until the viewer takes the offered refresh.
Note `close_group` does **not** bump `last_active` (only `reopen` does), which is exactly
why close/reopen must be caught by the `closed?` structural signal rather than the version
guard. Live updates are a **pure progressive enhancement**: they require htmx/JS, and if
the script never loads the app behaves exactly as before (manual refresh) — nothing
regresses. Sub-second latency is not available without moving to SSE/WebSocket.
**Deferred within this slice:** live-updating the pre-join *claim* screen's "N people in ·
tab total" social proof — an easy follow-up using the same endpoint pattern, left out so
v1 ships the thing that matters: people at the table watching the tab move.

**Revisit trigger:** htmx was pressure-tested against replacing it and consciously kept —
the whole live-updates design maps onto core htmx (polling, OOB swaps, `HX-Refresh`, 204),
so htmx is not the constraint here; the DOM layout and the stateless-polling choice are,
and both are framework-independent. Reconsider the client stack **only if** we decide we
want (a) sub-second server *push* (SSE/WebSocket) as the primary transport — then a
push-first hypermedia framework (Datastar / Hotwire Turbo) fits better than htmx-core +
the SSE extension — or (b) genuinely app-like client interactivity (optimistic UI,
live-validating forms, collaborative cursors). Absent one of those, a swap only spends the
one-script / no-build / small-download properties htmx was chosen for in decision #3.

**Captured from the live-updates grill on 2026-07-05.**

## 11. Add-expense is its own screen; the group page is read-only

**Decision:** "Add expense" is a **dedicated page** at `GET /g/{id}/add` (frame 04 of the
redesign) rather than a form embedded in the group page. The sticky button on the group
page navigates there; the form still posts to the existing `POST /g/{id}/expenses` and
redirects back on success. The group page itself is now **entirely read-only** — settle-up
hero, balances, expense/payment logs, invite, owner controls — with no input beyond the
owner's recovery/close controls.

**Why:** The design draws add-expense as a focused, full-screen task (big Total display,
one thing at a time), which is the right shape for filling it at a bar table. Making it a
route also **simplifies decision #10**: the live poller's careful "never swap the
add-expense form" carve-out becomes trivially safe because the group page holds no
in-progress input to clobber — every region there is fair game for an out-of-band swap.
The `frag_*` / `live_update` split is unchanged; the form simply no longer lives among the
swappable regions.

**Consequence:** Adding an expense is one navigation away instead of an in-page scroll, and
(under htmx `hx-boost`) it's an AJAX body swap; without JS it's an ordinary page load —
both work, consistent with #3. The "Who paid?" control stays a native `<select>` (name +
chevron) rather than the mockup's avatar-decorated custom dropdown: the avatar can't track
the selection without JS, and a native select keeps the screen accessible and scalable to
large rosters. A closed group renders no button and the route redirects back, so the
read-only invariant holds.

**Captured while implementing the "Dark · blue" redesign on 2026-07-05.**

---

## 12. htmx is vendored and self-served, not loaded from a CDN

**Decision:** The htmx runtime is **checked into the repo** at `assets/htmx-2.0.4.min.js`,
compiled into the binary with `include_str!`, and served from our own origin at
`GET /assets/htmx-2.0.4.min.js` (`Content-Type: text/javascript`, `Cache-Control: public,
max-age=31536000, immutable`). The layout's `<script>` points there instead of at
`unpkg.com`. The version is pinned **in the URL**, so bumping htmx changes the path and
busts client caches for free — which is what makes the year-long `immutable` cache safe.

**Why:** A CDN `<script>` is a third-party runtime dependency on every page: it can be
slow, blocked, or unavailable, and it forecloses a strict Content-Security-Policy (you'd
have to allow an external script origin). Self-serving removes the external dependency
(the app works offline / on a locked-down network), keeps the single-binary deploy story
intact — the asset ships *inside* the binary, so there's no static-file volume or extra
Docker `COPY` — and mirrors how the app already embeds its CSS, inline JS, and logo as
compiled-in constants (#3). Retires the "vendor it if you need a strict CSP or offline
use" caveat from the README's v1 limitations.

**Consequence:** Upgrading htmx is a three-touch change — drop the new file in `assets/`,
update the route path, update the `<script src>` — kept in sync by the version living in
the filename. A strict CSP is now unblocked but **not yet added**: the app still relies on
inline `<style>`/`<script>` and inline SVG, so a real CSP needs `'unsafe-inline'` or
nonces first. htmx remains a progressive enhancement per #3; the app is fully functional
if the script never loads. Integrity of the vendored file was confirmed byte-identical
across unpkg and jsdelivr (sha256 `e209dda5…fb447`).

**Captured on 2026-07-05.**

---

## 13. In-place expense editing; "rebalance on join" is a manual re-split

**Decision:** An expense can be **edited in place** at `GET`/`POST /g/{id}/expenses/{eid}/edit`
— a prefilled twin of the add-expense screen that rewrites the payer, total, description,
participant set, and per-share amounts. The edit **replaces the `expense_shares` snapshot**
in one transaction (update the row, delete the old shares, insert the new ones). Permission
matches delete: the **original payer or the owner**. Editing is refused on a **closed**
group (like adding). This retires the "delete + re-add" workaround from decision #6.

Pulling a **newly-joined member into a past expense** ("rebalance when new people join") is
**this same edit, done by hand**: open the round they should have been in, tick them, save.
There is deliberately **no join-triggered auto-rebalance** — the per-expense share snapshot
(#6) is what makes the 7pm round stay off the 9pm arrival's tab, and a blanket retroactive
re-split would violate that. New expenses already include newcomers automatically, so the
only surface that needed a tool was the retroactive one, and manual edit is it.

**Why:** Editing was always the planned next slice (#6); the snapshot model makes it a clean
replace rather than a schema change. Keeping "rebalance on join" as manual editing respects a
deliberate design decision and is strictly less surprising than firing changes on other
people's ledgers when someone walks in.

**Consequence:** An edit is allowed **even after settlements are recorded**, and balances
simply recompute from the new shares plus the existing settlements — so an edit can push a
recorded payment above the new outstanding debt and surface a reverse balance (e.g. someone
is now *owed* what they paid). This matches the app's forgiving, trust-based posture and the
fact that soft-delete already recomputes balances the same way; `mark_settlement` still
clamps at settlement time, but a later edit is not re-clamped. The original **equal-vs-exact
choice isn't persisted**, so the edit form defaults to **"Exact amounts"** with each stored
share filled in (always faithful); re-splitting equally is one radio tap. Live updates need
nothing new: an edit bumps `last_active`, so the existing poll (#10) repaints the read-only
regions via the same OOB swaps as add/delete/settle.

**Captured while implementing edit on 2026-07-08.**

---

## Explicitly out of scope for v1

Real accounts · itemized / percentage splits · multiple payers per expense · multi-currency
· monthly-period reporting · notifications · Swish / payment integration.

## Implementation choices (settled at build time, not blockers)

- **Storage:** SQLite via `sqlx` (async, trivial deploy). Amounts stored as integer minor
  units (öre) to avoid float error.
- **Templating:** `maud`.
- **Tokens:** random secret per owner/guest, stored **hashed** server-side, raw value in a
  cookie.
- Auto-expiry window (N days) and hosting target: TBD.
