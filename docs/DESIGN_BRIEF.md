# SettleUp — Design Handoff

A brief for a design-focused pass on SettleUp. It gives you the product, the users,
the domain, the surfaces to design, and the technical medium you must design **into** —
but deliberately **not** the current visual design. The existing look is not a
constraint; treat the styling in `src/views.rs` as a placeholder and redesign freely.

For the deeper "why" behind every product decision, read [`DECISIONS.md`](DECISIONS.md).

---

## 1. What it is

A tiny web app for splitting shared expenses. One person creates a **group**, everyone
else joins by scanning a QR code or opening a link — **no accounts, no install**. Each
person adds what they paid; the app computes the **fewest payments** that settle everyone
up, and each suggested payment can be marked as paid.

## 2. Who it's for — two use cases in tension

Every design decision is pulled between these two, and resolving the collision *is* the
product:

- **Bar tab** — a few strangers, in person, split a tab, then abandon it within the hour.
  Optimizes for: *strangers can join in seconds, zero friction, on their phones.*
- **Monthly split** — two known people (e.g. partners) accumulate shared expenses over a
  month and settle recurringly, wanting the **history to persist**.

They disagree on identity and lifetime. The product resolves it by making groups
**persistent by default**, throwaway ones **auto-clean-up**, and the only "I care about
this" signal is whether the owner set a recovery passphrase.

**Primary device is a phone.** The canonical entry is scanning a QR at a table.
Design mobile-first, one-handed, on cellular. Desktop is secondary.

## 3. Vocabulary (use these exact terms in UI copy)

- **Group** — the shared ledger: a name, a currency, members, expenses, settlements.
- **Owner** — the creator. Extra powers: delete any expense, close/reopen, set recovery.
- **Member / guest** — anyone who joined. Identity is a per-device token in a cookie —
  no login. Same person on two phones = two members.
- **Expense** — one **payer** fronts a **total** for a **description**, split across a
  chosen **subset** of members, either **equally** or by **exact** amounts.
- **Share** — one member's portion of one expense.
- **Balance / net** — what the group owes a member. Positive = owed money (creditor);
  negative = owes money (debtor). Nets always sum to zero.
- **Transfer** — a *suggested* payment ("A pays B X") produced by simplification. Not yet
  real money — it becomes real when someone marks it paid.
- **Settlement** — a recorded payment (A paid B back). Has an audit trail.
- **Close / reopen** — owner archives a squared-up group; reopenable later.
- **Recovery passphrase** — optional owner escape hatch to re-claim the group on a new
  device. Its presence also = "keep this group forever" (exempts it from auto-expiry).

## 4. Surfaces to design

There are only four rendered surfaces (all in `src/views.rs`):

1. **Landing / create** (`GET /`) — one form: group name, your name, currency (default
   `SEK`). CTA creates the group and drops you into it as owner.
2. **Claim / join** (`GET /g/{id}` when you have no cookie for it) — the QR/link
   destination. One field: your name → join. If the group has recovery, also offer an
   "Owner? Recover access" path.
3. **Recover** (`GET /g/{id}/recover`) — one passphrase field to re-claim owner access on
   a new device. Has an error state ("that passphrase didn't match").
4. **Group page** (`GET /g/{id}` when you're a member) — the heart of the app, and the
   real design challenge. It has to carry all of:
   - **Identity / invite**: group name, a QR code + copyable join link (server-rendered
     inline SVG), member list (owner badged).
   - **Settle-up (the hero)**: the suggested **transfers** — "these N payments square the
     group" — each with a **Mark paid** action. This is the headline value; make it the
     thing the eye lands on. When everyone's even, show a settled/zero state instead.
   - **Balances**: each member's net (owed vs owes), color-coded by sign.
   - **Add expense**: payer picker, amount, description, **equal-vs-exact toggle**, and
     **per-member subset selection** (checkboxes for equal; per-member amount fields for
     exact). This form must be *fast* to fill on a phone at a table.
   - **Expense log**: newest-first list — payer, amount, description, participants, time;
     a **delete** control visible only on rows you may delete (your own, or any if owner).
   - **Settlement log**: newest-first record of payments made.
   - **Owner controls**: set recovery passphrase; **close** (and, once closed, **reopen**).

## 5. States each surface must handle

- **Not yet joined** vs **joined** (claim screen vs full group page).
- **Empty group** — no expenses yet; the add-expense form and the invite are what matter.
- **Owes money** vs **fully settled** (no transfers) — very different group-page moods.
- **Open** vs **closed** — closed groups reject new expenses/settlements; surface the
  archived state and the reopen path (owner only).
- **Owner** vs **guest** — owner-only controls (delete-any, close/reopen, recovery) must
  be absent, not just disabled, for guests.
- **Has recovery ("kept")** vs **throwaway** — throwaway groups auto-delete after **3 days
  of inactivity**. Consider surfacing "this tab disappears unless you keep it," and make
  "set a recovery passphrase" read as *keep this group*.
- **Recovery error** — wrong passphrase.
- **Delete-permission** — per-row: delete control only where allowed.

## 6. Technical medium you're designing into (hard constraints)

This bounds what designs are buildable. None of it is about how it *looks* — it's the
substrate.

- **Server-rendered HTML, no SPA, no JS build step.** All markup is [`maud`](https://maud.lambda.xyz)
  templates in `src/views.rs`; **all CSS is a single `STYLES` string constant** injected
  into `<head>` in that same file. A redesign = editing that Rust file's markup + that CSS
  string. There is no asset pipeline, no CSS framework, no components dir.
- **Must work with JavaScript disabled.** Every action is a plain `<form>` POST that
  redirects back to the group page. [`htmx`](https://htmx.org) (v2, from a CDN, via
  `hx-boost` on `<body>`) is a *progressive enhancement* that AJAX-swaps navigations —
  nothing may *depend* on it. No client-side state, no SPA patterns.
- **Self-contained / CSP-friendly.** The app currently pulls htmx from a CDN, but the goal
  is to be vendorable for a strict Content-Security-Policy and offline use. Prefer
  **system fonts** or self-hostable assets; avoid mandatory external fonts/CDNs. Embed
  icons/images inline (e.g. as SVG or data URIs) rather than remote fetches.
- **QR code** is generated server-side as inline SVG — you style its container, not its
  contents.
- **Money**: stored as integer öre; displayed as two-decimal strings (e.g. `125.50`).
  Input accepts both `.` and `,` as the decimal separator (Swedish keyboards). One
  currency per group, default **SEK**, no conversion.
- **Responsive**: single column, ~640px max content width is the current assumption;
  phone-first. You may change the shell, but keep it a single fluid column that works at
  360px wide.

## 7. Tone / context

Swedish context (SEK default; Swish deep-links off "Mark paid" are a planned v2, so the
settlement action is a natural future payment launch point). It's a **money app among
friends/strangers** — should feel quick and trustworthy, not corporate or heavy. The bar
case wants *fun and instant*; the monthly case wants *calm and legible*. A good design
serves both without a mode switch.

## 8. Explicitly out of scope (don't design for these)

Real accounts · itemized/percentage/share splits · multiple payers per expense ·
multi-currency · monthly-period reporting/statements · notifications · in-place expense
editing (v1 is delete-and-re-add) · payment-provider integration (Swish is v2-designed-for,
not built).

## 9. Where to make changes

Everything visual lives in **`src/views.rs`**:
- `STYLES` — the single CSS constant.
- `layout(title, body)` — the page shell (`<head>`, htmx include, body wrapper).
- `landing()`, `claim()`, `recover()`, `group_page()` — the four surfaces, plus the small
  `*Row` structs that carry the already-computed display data (names, formatted-ready
  amounts as öre, permission flags) into the group page.

The handlers (`src/handlers.rs`), math (`src/settle.rs`), and data model
(`src/db.rs`, `src/models.rs`) define *what data exists* on each surface; you shouldn't
need to change them for a visual redesign. If a design needs a new piece of data or a new
state, that's a conversation, not a silent assumption.
