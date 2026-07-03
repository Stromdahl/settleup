//! Randomized "bar tab" simulator that stress-tests the settle-up math.
//!
//! It generates thousands of plausible scenarios (a handful of people, several
//! rounds of drinks split equally or in exact amounts across a subset, plus the
//! odd partial payback) with a *seeded* RNG, then asserts every invariant the
//! whole app leans on:
//!
//! - money is conserved: what payers fronted equals what participants owe;
//! - `equal_shares` loses no öre and spreads the remainder to the earliest members;
//! - net balances always sum to exactly zero;
//! - the simplified transfers actually settle everyone to zero, each debtor pays
//!   exactly their debt, each creditor receives exactly their due, and members with
//!   a zero balance never appear in a transfer;
//! - `simplify` is deterministic and never emits more than `nonzero - 1` transfers.
//!
//! Every scenario is derived purely from its seed, so a failure prints the seed and
//! the offending scenario and can be replayed exactly.
//!
//! Run just this module and watch it work:
//!
//! ```sh
//! cargo test sim -- --nocapture
//! ```

use crate::money::{format_amount, parse_amount};
use crate::settle::{equal_shares, net_balances, simplify};

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Method {
    Equal,
    Exact,
}

/// One round on the tab: who paid, how it was split, and the per-member shares
/// (in öre) which by construction sum to the expense total — exactly the shape the
/// `add_expense` handler persists.
#[derive(Clone, Debug, PartialEq)]
struct Expense {
    payer: i64,
    method: Method,
    desc: &'static str,
    shares: Vec<(i64, i64)>,
}

impl Expense {
    fn total(&self) -> i64 {
        self.shares.iter().map(|(_, a)| a).sum()
    }
}

/// A whole generated tab: the members, the rounds, and any partial paybacks.
#[derive(Clone, Debug, PartialEq)]
struct Scenario {
    seed: u64,
    members: Vec<i64>,
    expenses: Vec<Expense>,
    settlements: Vec<(i64, i64, i64)>,
}

impl Scenario {
    /// `(payer, total)` per expense — the "fronted" side of the ledger.
    fn payments(&self) -> Vec<(i64, i64)> {
        self.expenses.iter().map(|e| (e.payer, e.total())).collect()
    }
    /// Every `(member, share)` pair across all expenses — the "owed" side.
    fn shares(&self) -> Vec<(i64, i64)> {
        self.expenses
            .iter()
            .flat_map(|e| e.shares.iter().copied())
            .collect()
    }
    fn balances(&self) -> Vec<(i64, i64)> {
        net_balances(&self.members, &self.payments(), &self.shares(), &self.settlements)
    }
}

const DESCS: [&str; 8] = [
    "a round of beers",
    "nachos",
    "shots",
    "the cover charge",
    "a bottle of wine",
    "cocktails",
    "the taxi home",
    "wings",
];

/// Build a scenario deterministically from `seed`. Same seed → identical scenario.
fn gen_scenario(seed: u64) -> Scenario {
    let mut rng = StdRng::seed_from_u64(seed);

    // 2..=8 people at the table, with deliberately non-contiguous ids so nothing
    // downstream may quietly assume ids are 0-based or dense.
    let n = rng.random_range(2..=8usize);
    let mut members = Vec::with_capacity(n);
    let mut id = rng.random_range(1..50i64);
    for _ in 0..n {
        members.push(id);
        id += rng.random_range(1..5i64);
    }

    let num_expenses = rng.random_range(1..=12usize);
    let mut expenses = Vec::with_capacity(num_expenses);
    for _ in 0..num_expenses {
        // The payer can be anyone — at a bar you sometimes buy a round you're not in.
        let payer = members[rng.random_range(0..n)];

        // Split across a random non-empty subset; mirror the handler's `sort()`.
        let mut subset: Vec<i64> = members
            .iter()
            .copied()
            .filter(|_| rng.random_bool(0.6))
            .collect();
        if subset.is_empty() {
            subset.push(members[rng.random_range(0..n)]);
        }
        subset.sort_unstable();

        let desc = DESCS[rng.random_range(0..DESCS.len())];
        let (method, shares) = if rng.random_bool(0.5) {
            // Equal: pick a total (0.01..=5000.00) and let equal_shares divide it —
            // exactly what the handler does.
            let total = rng.random_range(1..=500_000i64);
            (Method::Equal, equal_shares(total, &subset))
        } else {
            // Exact: pick a positive amount per member; the total is their sum.
            let shares = subset
                .iter()
                .map(|&m| (m, rng.random_range(1..=100_000i64)))
                .collect();
            (Method::Exact, shares)
        };
        expenses.push(Expense {
            payer,
            method,
            desc,
            shares,
        });
    }

    // 0..=3 partial paybacks between two distinct members. These can be any amount
    // (a user may mark a partial or over-payment); the math must cope regardless.
    let num_settle = rng.random_range(0..=3usize);
    let mut settlements = Vec::with_capacity(num_settle);
    for _ in 0..num_settle {
        let a = rng.random_range(0..n);
        let mut b = rng.random_range(0..n);
        while b == a {
            b = rng.random_range(0..n);
        }
        let amount = rng.random_range(1..=200_000i64);
        settlements.push((members[a], members[b], amount));
    }

    Scenario {
        seed,
        members,
        expenses,
        settlements,
    }
}

/// Assert every invariant for one scenario. Panics carry the seed + full scenario
/// so any failure is reproducible via `gen_scenario(seed)`.
fn check_scenario(s: &Scenario) {
    let ctx = || format!("seed={} {:#?}", s.seed, s);

    let payments = s.payments();
    let shares = s.shares();

    // --- money conservation (also guards that the generator mirrors the handler) ---
    let paid: i64 = payments.iter().map(|(_, a)| a).sum();
    let owed: i64 = shares.iter().map(|(_, a)| a).sum();
    assert_eq!(paid, owed, "money not conserved: paid != owed\n{}", ctx());

    // --- per-expense split correctness ---
    for e in &s.expenses {
        let total = e.total();
        assert!(total > 0, "expense total must be positive\n{}", ctx());
        for &(_, a) in &e.shares {
            assert!(a >= 0, "no negative shares\n{}", ctx());
        }
        if e.method == Method::Equal {
            let cnt = e.shares.len() as i64;
            let base = total / cnt;
            let rem = total - base * cnt;
            let min = e.shares.iter().map(|(_, a)| *a).min().unwrap();
            let max = e.shares.iter().map(|(_, a)| *a).max().unwrap();
            assert!(max - min <= 1, "equal shares differ by more than 1 öre\n{}", ctx());
            // The first `rem` (lowest-id) members carry the leftover öre.
            for (i, &(_, a)) in e.shares.iter().enumerate() {
                let expect = base + if (i as i64) < rem { 1 } else { 0 };
                assert_eq!(a, expect, "equal remainder mis-distributed\n{}", ctx());
            }
        }
    }

    let balances = s.balances();

    // Balances cover exactly the members, in member order.
    assert_eq!(
        balances.iter().map(|(m, _)| *m).collect::<Vec<_>>(),
        s.members,
        "balances must cover every member in order\n{}",
        ctx()
    );
    // Nets always sum to zero.
    let net_sum: i64 = balances.iter().map(|(_, b)| b).sum();
    assert_eq!(net_sum, 0, "net balances don't sum to zero\n{}", ctx());

    let transfers = simplify(&balances);
    let bmap: HashMap<i64, i64> = balances.iter().copied().collect();

    // No more transfers than `nonzero - 1`.
    let nonzero = balances.iter().filter(|(_, b)| *b != 0).count();
    assert!(
        transfers.len() <= nonzero.saturating_sub(1),
        "too many transfers ({} > {})\n{}",
        transfers.len(),
        nonzero.saturating_sub(1),
        ctx()
    );

    // Each transfer is well-formed: positive, debtor -> creditor, no self-pay.
    for t in &transfers {
        assert!(t.amount > 0, "transfer amount must be positive\n{}", ctx());
        assert_ne!(t.from, t.to, "no self-transfer\n{}", ctx());
        assert!(bmap.get(&t.from).copied().unwrap_or(0) < 0, "payer isn't a debtor\n{}", ctx());
        assert!(bmap.get(&t.to).copied().unwrap_or(0) > 0, "payee isn't a creditor\n{}", ctx());
    }

    // Applying every transfer settles the whole group to exactly zero.
    let mut net = bmap.clone();
    for t in &transfers {
        *net.get_mut(&t.from).unwrap() += t.amount;
        *net.get_mut(&t.to).unwrap() -= t.amount;
    }
    assert!(net.values().all(|&v| v == 0), "transfers don't settle everyone\n{}", ctx());

    // Each debtor pays exactly their debt, each creditor receives exactly their due,
    // and a zero-balance member neither pays nor receives anything.
    let mut sent: HashMap<i64, i64> = HashMap::new();
    let mut recv: HashMap<i64, i64> = HashMap::new();
    for t in &transfers {
        *sent.entry(t.from).or_default() += t.amount;
        *recv.entry(t.to).or_default() += t.amount;
    }
    for (&m, &b) in &bmap {
        let s_out = sent.get(&m).copied().unwrap_or(0);
        let r_in = recv.get(&m).copied().unwrap_or(0);
        if b < 0 {
            assert_eq!(s_out, -b, "debtor must pay exactly their debt\n{}", ctx());
            assert_eq!(r_in, 0, "debtor must not receive\n{}", ctx());
        } else if b > 0 {
            assert_eq!(r_in, b, "creditor must receive exactly their due\n{}", ctx());
            assert_eq!(s_out, 0, "creditor must not pay\n{}", ctx());
        } else {
            assert_eq!(s_out, 0, "zero-balance member must not pay\n{}", ctx());
            assert_eq!(r_in, 0, "zero-balance member must not receive\n{}", ctx());
        }
    }

    // Determinism: simplify is a pure function of the balances.
    assert_eq!(transfers, simplify(&balances), "simplify not deterministic\n{}", ctx());
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The main workhorse: generate a bunch of tabs and assert every invariant.
#[test]
fn fuzz_bar_scenarios() {
    const N: u64 = 50_000;
    for seed in 0..N {
        check_scenario(&gen_scenario(seed));
    }
}

/// `equal_shares` conserves the total, spreads the remainder to the earliest
/// members, never differs by more than 1 öre, and keeps members in order —
/// including the awkward cases where the total is smaller than the headcount.
#[test]
fn equal_shares_properties() {
    let mut rng = StdRng::seed_from_u64(0xE9_1A);
    for _ in 0..100_000 {
        let n = rng.random_range(1..=12i64);
        let members: Vec<i64> = (1..=n).collect();
        let total = rng.random_range(0..=1_000_000i64);
        let shares = equal_shares(total, &members);

        assert_eq!(shares.len(), n as usize);
        assert_eq!(shares.iter().map(|(_, a)| a).sum::<i64>(), total, "must conserve total");
        assert_eq!(
            shares.iter().map(|(m, _)| *m).collect::<Vec<_>>(),
            members,
            "members preserved and in order"
        );

        let base = total / n;
        let rem = total - base * n;
        for (i, &(_, a)) in shares.iter().enumerate() {
            assert!(a >= 0, "no negative share");
            let expect = base + if (i as i64) < rem { 1 } else { 0 };
            assert_eq!(a, expect, "remainder to earliest members");
        }
    }
    assert!(equal_shares(100, &[]).is_empty(), "empty membership yields no shares");
}

/// Money survives a `format_amount` -> `parse_amount` round trip for every value.
#[test]
fn money_roundtrips() {
    let mut rng = StdRng::seed_from_u64(7);
    for _ in 0..200_000 {
        let x = rng.random_range(0..1_000_000_000i64);
        assert_eq!(parse_amount(&format_amount(x)), Some(x), "roundtrip failed for {x}");
    }
    assert_eq!(parse_amount(&format_amount(0)), Some(0));
    assert_eq!(parse_amount(&format_amount(5)), Some(5)); // 0.05
}

/// Same seed must reproduce the same scenario and the same suggested transfers.
#[test]
fn generation_is_deterministic() {
    for seed in [0u64, 1, 7, 42, 999, 123_456] {
        let a = gen_scenario(seed);
        let b = gen_scenario(seed);
        assert_eq!(a, b, "scenario not reproducible for seed {seed}");
        assert_eq!(simplify(&a.balances()), simplify(&b.balances()));
    }
}

/// Human-readable sample tabs so a person can eyeball that the numbers are sane.
/// Visible with `cargo test sim -- --nocapture`.
#[test]
fn demo_prints_sample_bar_tabs() {
    const NAMES: [&str; 8] = ["Alice", "Bob", "Cara", "Dan", "Eve", "Finn", "Gwen", "Hugo"];

    println!("\n=== Sample generated bar tabs (--nocapture to see) ===");
    for seed in [1u64, 7, 42, 100, 2024] {
        let s = gen_scenario(seed);
        check_scenario(&s); // the demo doubles as a check
        let name: HashMap<i64, &str> = s.members.iter().enumerate().map(|(i, &m)| (m, NAMES[i])).collect();

        println!(
            "\nBar tab #{} — {}:",
            seed,
            s.members.iter().map(|m| name[m]).collect::<Vec<_>>().join(", ")
        );
        for e in &s.expenses {
            let split = match e.method {
                Method::Equal => format!(
                    "split equally among {}",
                    e.shares.iter().map(|(m, _)| name[m]).collect::<Vec<_>>().join(", ")
                ),
                Method::Exact => format!(
                    "exact: {}",
                    e.shares
                        .iter()
                        .map(|(m, a)| format!("{} {}", name[m], format_amount(*a)))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            println!("  {:>5} paid {:>10} for {} ({split})", name[&e.payer], format_amount(e.total()), e.desc);
        }
        for &(from, to, amt) in &s.settlements {
            println!("  payback: {} → {} {}", name[&from], name[&to], format_amount(amt));
        }
        println!("  balances:");
        for (m, b) in s.balances() {
            let owed = if b > 0 {
                format!("is owed {}", format_amount(b))
            } else if b < 0 {
                format!("owes {}", format_amount(-b))
            } else {
                "square".to_string()
            };
            println!("    {:>5}: {}", name[&m], owed);
        }
        let transfers = simplify(&s.balances());
        if transfers.is_empty() {
            println!("  settle up: nothing to do");
        } else {
            println!("  settle up with {} payment(s):", transfers.len());
            for t in &transfers {
                println!("    {} → {} {}", name[&t.from], name[&t.to], format_amount(t.amount));
            }
        }
    }

    // Aggregate feel for how many transfers the greedy simplifier produces.
    let runs = 20_000u64;
    let (mut total, mut max) = (0usize, 0usize);
    for seed in 1_000_000..1_000_000 + runs {
        let t = simplify(&gen_scenario(seed).balances()).len();
        total += t;
        max = max.max(t);
    }
    println!(
        "\nOver {runs} tabs: {:.2} transfers on average, {max} at most.",
        total as f64 / runs as f64
    );
}
