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
