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
use std::time::{Duration, Instant};

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

// ===========================================================================
// Campaign — a time-boxed, multi-threaded soak that sweeps many configurations
// and runs deeper experiments. Ignored by default (it runs for minutes); launch
// it explicitly in release for real throughput:
//
//   SETTLEUP_SIM_SECS=1200 cargo test --release sim_campaign -- --ignored --nocapture
//
// Env:  SETTLEUP_SIM_SECS   total wall-clock budget in seconds (default 1200)
//       SETTLEUP_SIM_THREADS number of worker threads     (default: CPU cores)
//
// Every scenario still derives purely from a seed, so any failure prints the
// seed (or the offending balance vector) and can be replayed.
// ===========================================================================

/// A named generation profile — one "constellation" to sweep.
#[derive(Clone, Copy)]
struct Cfg {
    name: &'static str,
    min_m: usize,
    max_m: usize,
    max_exp: usize,
    amt_max: i64,
    equal_bias: f64,
    density: f64,
    settle_max: usize,
    host_skew: bool,
    delete_prob: f64,
}

/// The sweep: small bar tabs through 800-person groups, öre-tiny through
/// near-astronomical amounts, all-equal / all-exact, sparse subsets, and a
/// "one generous host pays for everything" skew.
const CONFIGS: [Cfg; 10] = [
    Cfg { name: "bar-small",      min_m: 2,   max_m: 8,   max_exp: 12,  amt_max: 500_000,               equal_bias: 0.5, density: 0.6, settle_max: 3,  host_skew: false, delete_prob: 0.10 },
    Cfg { name: "bar-large",      min_m: 8,   max_m: 30,  max_exp: 60,  amt_max: 500_000,               equal_bias: 0.5, density: 0.5, settle_max: 10, host_skew: false, delete_prob: 0.10 },
    Cfg { name: "big-group",      min_m: 30,  max_m: 300, max_exp: 150, amt_max: 300_000,               equal_bias: 0.6, density: 0.3, settle_max: 20, host_skew: false, delete_prob: 0.05 },
    Cfg { name: "huge-group",     min_m: 300, max_m: 800, max_exp: 60,  amt_max: 100_000,               equal_bias: 0.6, density: 0.1, settle_max: 20, host_skew: false, delete_prob: 0.05 },
    Cfg { name: "tiny-amounts",   min_m: 2,   max_m: 12,  max_exp: 20,  amt_max: 50,                    equal_bias: 0.8, density: 0.8, settle_max: 3,  host_skew: false, delete_prob: 0.05 },
    Cfg { name: "huge-amounts",   min_m: 2,   max_m: 12,  max_exp: 30,  amt_max: 1_000_000_000_000_000, equal_bias: 0.5, density: 0.6, settle_max: 3,  host_skew: false, delete_prob: 0.05 },
    Cfg { name: "all-equal",      min_m: 2,   max_m: 16,  max_exp: 30,  amt_max: 500_000,               equal_bias: 1.0, density: 0.6, settle_max: 5,  host_skew: false, delete_prob: 0.05 },
    Cfg { name: "all-exact",      min_m: 2,   max_m: 16,  max_exp: 30,  amt_max: 500_000,               equal_bias: 0.0, density: 0.6, settle_max: 5,  host_skew: false, delete_prob: 0.05 },
    Cfg { name: "sparse-subsets", min_m: 4,   max_m: 20,  max_exp: 40,  amt_max: 400_000,               equal_bias: 0.5, density: 0.2, settle_max: 5,  host_skew: false, delete_prob: 0.05 },
    Cfg { name: "generous-host",  min_m: 3,   max_m: 12,  max_exp: 40,  amt_max: 500_000,               equal_bias: 0.5, density: 0.7, settle_max: 5,  host_skew: true,  delete_prob: 0.05 },
];

// Groups kept small (n <= 12) so the exact optimum stays computable (k <= 12).
const ORACLE_CFG: Cfg = Cfg { name: "oracle", min_m: 2, max_m: 12, max_exp: 14, amt_max: 5_000,     equal_bias: 0.5, density: 0.7, settle_max: 4, host_skew: false, delete_prob: 0.0 };
const META_CFG: Cfg   = Cfg { name: "meta",   min_m: 2, max_m: 20, max_exp: 30, amt_max: 1_000_000, equal_bias: 0.5, density: 0.5, settle_max: 6, host_skew: false, delete_prob: 0.0 };
const RT_CFG: Cfg     = Cfg { name: "rt",     min_m: 2, max_m: 30, max_exp: 40, amt_max: 500_000,   equal_bias: 0.5, density: 0.5, settle_max: 8, host_skew: false, delete_prob: 0.05 };

/// Parameterized generator (the sweep version of `gen_scenario`). Mirrors the
/// `add_expense` handler: equal splits go through `equal_shares(total, subset)`,
/// exact splits pick a positive amount per member. `delete_prob` folds in the
/// app's delete-only editing by dropping some expenses (added-then-deleted).
fn gen_scenario_cfg(seed: u64, cfg: &Cfg) -> Scenario {
    let mut rng = StdRng::seed_from_u64(seed);

    let n = rng.random_range(cfg.min_m..=cfg.max_m);
    let mut members = Vec::with_capacity(n);
    let mut id = rng.random_range(1..50i64);
    for _ in 0..n {
        members.push(id);
        id += rng.random_range(1..5i64);
    }

    let num_exp = rng.random_range(1..=cfg.max_exp);
    let mut expenses = Vec::with_capacity(num_exp);
    for _ in 0..num_exp {
        if rng.random_bool(cfg.delete_prob) {
            continue; // this round was added then deleted — net effect none
        }
        let payer = if cfg.host_skew && rng.random_bool(0.8) {
            members[0]
        } else {
            members[rng.random_range(0..n)]
        };
        let mut subset: Vec<i64> = members
            .iter()
            .copied()
            .filter(|_| rng.random_bool(cfg.density))
            .collect();
        if subset.is_empty() {
            subset.push(members[rng.random_range(0..n)]);
        }
        subset.sort_unstable();
        let desc = DESCS[rng.random_range(0..DESCS.len())];
        let (method, shares) = if rng.random_bool(cfg.equal_bias) {
            let total = rng.random_range(1..=cfg.amt_max);
            (Method::Equal, equal_shares(total, &subset))
        } else {
            let shares = subset
                .iter()
                .map(|&m| (m, rng.random_range(1..=cfg.amt_max)))
                .collect();
            (Method::Exact, shares)
        };
        expenses.push(Expense { payer, method, desc, shares });
    }
    if expenses.is_empty() {
        expenses.push(Expense {
            payer: members[0],
            method: Method::Exact,
            desc: DESCS[0],
            shares: vec![(members[0], 100)],
        });
    }

    let ns = rng.random_range(0..=cfg.settle_max);
    let mut settlements = Vec::with_capacity(ns);
    for _ in 0..ns {
        let a = rng.random_range(0..n);
        let mut b = rng.random_range(0..n);
        while b == a {
            b = rng.random_range(0..n);
        }
        let amount = rng.random_range(1..=cfg.amt_max.min(200_000));
        settlements.push((members[a], members[b], amount));
    }

    Scenario { seed, members, expenses, settlements }
}

/// Exact minimum number of transfers to settle nonzero balances `bal`.
///
/// A set of people whose balances sum to zero can be internally squared in
/// `size - 1` transfers, so the minimum over the whole group is
/// `k - (max number of zero-sum parts we can partition the k people into)`.
/// The max-partition is a bitmask DP over subsets in O(3^k); anchoring each
/// part on the lowest remaining index enumerates every partition exactly once.
/// `sum`/`dp` are reusable scratch buffers of length >= 2^k.
fn min_transfers_optimal(bal: &[i64], sum: &mut [i64], dp: &mut [i32]) -> usize {
    let k = bal.len();
    if k == 0 {
        return 0;
    }
    let size = 1usize << k;
    sum[0] = 0;
    for mask in 1..size {
        let low = mask.trailing_zeros() as usize;
        sum[mask] = sum[mask & (mask - 1)] + bal[low];
    }
    for d in dp[..size].iter_mut() {
        *d = -1;
    }
    dp[0] = 0;
    for mask in 1..size {
        let low_bit = mask & mask.wrapping_neg(); // lowest set bit
        let mut sub = mask;
        let mut best = -1i32;
        while sub > 0 {
            if sub & low_bit != 0 && sum[sub] == 0 {
                let rest = dp[mask ^ sub];
                if rest >= 0 && rest + 1 > best {
                    best = rest + 1;
                }
            }
            sub = (sub - 1) & mask;
        }
        dp[mask] = best;
    }
    // The full set always sums to zero, so dp[full] >= 1.
    k - dp[size - 1] as usize
}

/// Number of transfers greedy `simplify` produces for a raw balance vector.
fn greedy_len(bal: &[i64]) -> usize {
    let pairs: Vec<(i64, i64)> = bal.iter().enumerate().map(|(i, &b)| (i as i64, b)).collect();
    simplify(&pairs).len()
}

/// A random integer vector of length `len` summing to exactly zero.
fn rand_zero_vec(rng: &mut StdRng, len: usize, r: i64) -> Vec<i64> {
    let mut v: Vec<i64> = (0..len - 1).map(|_| rng.random_range(-r..=r)).collect();
    let s: i64 = v.iter().sum();
    v.push(-s);
    v
}

/// Independent i128 re-derivation of balances (overflow detector for the i64 path).
fn balances_i128(
    members: &[i64],
    pay: &[(i64, i64)],
    sh: &[(i64, i64)],
    set: &[(i64, i64, i64)],
) -> HashMap<i64, i128> {
    let mut m: HashMap<i64, i128> = members.iter().map(|&x| (x, 0i128)).collect();
    for &(p, a) in pay {
        *m.entry(p).or_insert(0) += a as i128;
    }
    for &(x, a) in sh {
        *m.entry(x).or_insert(0) -= a as i128;
    }
    for &(f, t, a) in set {
        *m.entry(f).or_insert(0) += a as i128;
        *m.entry(t).or_insert(0) -= a as i128;
    }
    m
}

fn shuffle<T>(v: &mut [T], rng: &mut StdRng) {
    for i in (1..v.len()).rev() {
        let j = rng.random_range(0..=i);
        v.swap(i, j);
    }
}

/// Append one generated expense onto a running ledger (for the lifecycle sim).
fn gen_expense_into(
    rng: &mut StdRng,
    members: &[i64],
    amt_max: i64,
    pay: &mut Vec<(i64, i64)>,
    sh: &mut Vec<(i64, i64)>,
) {
    let n = members.len();
    let payer = members[rng.random_range(0..n)];
    let mut subset: Vec<i64> = members.iter().copied().filter(|_| rng.random_bool(0.6)).collect();
    if subset.is_empty() {
        subset.push(members[rng.random_range(0..n)]);
    }
    subset.sort_unstable();
    if rng.random_bool(0.5) {
        let total = rng.random_range(1..=amt_max);
        pay.push((payer, total));
        for (m, a) in equal_shares(total, &subset) {
            sh.push((m, a));
        }
    } else {
        let mut total = 0;
        for &m in &subset {
            let a = rng.random_range(1..=amt_max);
            total += a;
            sh.push((m, a));
        }
        pay.push((payer, total));
    }
}

// --- experiments (each returns human-readable report lines) --------------------

/// #1 Config-sweep fuzz: the full invariant check across every constellation.
fn exp_fuzz(deadline: Instant, threads: usize, base: u64) -> Vec<String> {
    #[derive(Default)]
    struct S { count: u64, max_m: usize, max_e: usize, max_t: usize }
    let parts: Vec<S> = std::thread::scope(|sc| {
        (0..threads)
            .map(|t| {
                sc.spawn(move || {
                    let mut st = S::default();
                    let mut seed = base.wrapping_add(t as u64);
                    while Instant::now() < deadline {
                        let c = CONFIGS[(seed as usize) % CONFIGS.len()];
                        let s = gen_scenario_cfg(seed, &c);
                        check_scenario(&s);
                        st.count += 1;
                        st.max_m = st.max_m.max(s.members.len());
                        st.max_e = st.max_e.max(s.expenses.len());
                        st.max_t = st.max_t.max(simplify(&s.balances()).len());
                        seed = seed.wrapping_add(threads as u64);
                    }
                    st
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect()
    });
    let count: u64 = parts.iter().map(|p| p.count).sum();
    vec![
        format!("configs swept: {}", CONFIGS.iter().map(|c| c.name).collect::<Vec<_>>().join(", ")),
        format!("{count} scenarios checked — every invariant held"),
        format!(
            "largest {} members / {} expenses; most transfers {}",
            parts.iter().map(|p| p.max_m).max().unwrap_or(0),
            parts.iter().map(|p| p.max_e).max().unwrap_or(0),
            parts.iter().map(|p| p.max_t).max().unwrap_or(0),
        ),
    ]
}

/// #6 Optimality oracle: greedy `simplify` vs the exact minimum.
fn exp_oracle(deadline: Instant, threads: usize, base: u64) -> Vec<String> {
    #[derive(Default, Clone)]
    struct S { count: u64, sub: u64, sum_ex: u64, max_ex: usize, w_seed: u64, w_g: usize, w_o: usize, skip: u64 }
    let parts: Vec<S> = std::thread::scope(|sc| {
        (0..threads)
            .map(|t| {
                sc.spawn(move || {
                    let mut st = S::default();
                    let mut sbuf = vec![0i64; 4096];
                    let mut dbuf = vec![0i32; 4096];
                    let mut seed = base.wrapping_add(t as u64);
                    while Instant::now() < deadline {
                        let s = gen_scenario_cfg(seed, &ORACLE_CFG);
                        let bal = s.balances();
                        let nz: Vec<i64> = bal.iter().map(|(_, b)| *b).filter(|&b| b != 0).collect();
                        if nz.len() > 12 {
                            st.skip += 1;
                            seed = seed.wrapping_add(threads as u64);
                            continue;
                        }
                        let opt = min_transfers_optimal(&nz, &mut sbuf, &mut dbuf);
                        let greedy = simplify(&bal).len();
                        assert!(
                            greedy >= opt,
                            "greedy {greedy} < optimal {opt} at seed={} — ORACLE/GREEDY BUG\n{:#?}",
                            s.seed, s
                        );
                        st.count += 1;
                        if greedy > opt {
                            let ex = greedy - opt;
                            st.sub += 1;
                            st.sum_ex += ex as u64;
                            if ex > st.max_ex {
                                st.max_ex = ex;
                                st.w_seed = s.seed;
                                st.w_g = greedy;
                                st.w_o = opt;
                            }
                        }
                        seed = seed.wrapping_add(threads as u64);
                    }
                    st
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect()
    });
    let count: u64 = parts.iter().map(|p| p.count).sum();
    let sub: u64 = parts.iter().map(|p| p.sub).sum();
    let sum_ex: u64 = parts.iter().map(|p| p.sum_ex).sum();
    let skip: u64 = parts.iter().map(|p| p.skip).sum();
    let worst = parts.iter().max_by_key(|p| p.max_ex).cloned().unwrap_or_default();
    let matched = if count > 0 { 100.0 * (count - sub) as f64 / count as f64 } else { 0.0 };
    let avg_ex = if sub > 0 { sum_ex as f64 / sub as f64 } else { 0.0 };
    vec![
        format!("{count} scenarios compared to the exact optimum (greedy >= optimum always held)"),
        format!("greedy hit the optimum in {matched:.3}% of cases ({sub} suboptimal)"),
        format!("when suboptimal: +{avg_ex:.2} extra transfers on average, +{} at most", worst.max_ex),
        if worst.max_ex > 0 {
            format!("worst case: greedy {} vs optimal {} (replay seed {})", worst.w_g, worst.w_o, worst.w_seed)
        } else {
            "greedy never beat the optimum by more than 0".into()
        },
        format!("({skip} skipped for k>12)"),
    ]
}

/// #7 Adversarial search: hill-climb balance vectors to maximise greedy's gap.
fn exp_adversarial(deadline: Instant, threads: usize, base: u64) -> Vec<String> {
    #[derive(Default, Clone)]
    struct S { restarts: u64, evals: u64, best: usize, g: usize, o: usize, vec: Vec<i64> }
    let parts: Vec<S> = std::thread::scope(|sc| {
        (0..threads)
            .map(|t| {
                sc.spawn(move || {
                    let mut st = S::default();
                    let mut rng = StdRng::seed_from_u64(base ^ (t as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                    let mut sbuf = vec![0i64; 4096];
                    let mut dbuf = vec![0i32; 4096];
                    let gap = |v: &[i64], sb: &mut [i64], db: &mut [i32]| -> usize {
                        let nz: Vec<i64> = v.iter().copied().filter(|&b| b != 0).collect();
                        if nz.len() > 12 {
                            return 0;
                        }
                        greedy_len(v).saturating_sub(min_transfers_optimal(&nz, sb, db))
                    };
                    while Instant::now() < deadline {
                        let len = rng.random_range(3..=10usize);
                        let mut cur = rand_zero_vec(&mut rng, len, 20);
                        let mut cur_gap = gap(&cur, &mut sbuf, &mut dbuf);
                        st.restarts += 1;
                        for _ in 0..80 {
                            if Instant::now() >= deadline {
                                break;
                            }
                            let mut cand = cur.clone();
                            let i = rng.random_range(0..len);
                            let mut j = rng.random_range(0..len);
                            while j == i {
                                j = rng.random_range(0..len);
                            }
                            let d = rng.random_range(1..=6i64);
                            cand[i] += d;
                            cand[j] -= d;
                            let g = gap(&cand, &mut sbuf, &mut dbuf);
                            st.evals += 1;
                            if g >= cur_gap {
                                cur = cand;
                                cur_gap = g;
                            }
                        }
                        if cur_gap > st.best {
                            let nz: Vec<i64> = cur.iter().copied().filter(|&b| b != 0).collect();
                            st.best = cur_gap;
                            st.g = greedy_len(&cur);
                            st.o = min_transfers_optimal(&nz, &mut sbuf, &mut dbuf);
                            st.vec = cur.clone();
                        }
                    }
                    st
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect()
    });
    let evals: u64 = parts.iter().map(|p| p.evals).sum();
    let restarts: u64 = parts.iter().map(|p| p.restarts).sum();
    let best = parts.iter().max_by_key(|p| p.best).cloned().unwrap_or_default();
    vec![
        format!("{restarts} restarts, {evals} candidate balance-vectors evaluated"),
        if best.best > 0 {
            format!("worst gap found: greedy {} vs optimal {} (+{}) for balances {:?}", best.g, best.o, best.best, best.vec)
        } else {
            "greedy stayed optimal on every vector tried".into()
        },
    ]
}

/// #3 Metamorphic properties — transforms that must not change balances.
fn exp_metamorphic(deadline: Instant, threads: usize, base: u64) -> Vec<String> {
    let parts: Vec<u64> = std::thread::scope(|sc| {
        (0..threads)
            .map(|t| {
                sc.spawn(move || {
                    let mut count = 0u64;
                    let mut seed = base.wrapping_add(t as u64);
                    while Instant::now() < deadline {
                        metamorphic_check(seed);
                        count += 1;
                        seed = seed.wrapping_add(threads as u64);
                    }
                    count
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect()
    });
    let count: u64 = parts.iter().sum();
    vec![
        format!("{count} scenarios × 5 transforms held"),
        "checked: order-independence, ×k linearity, self-paid no-op, settlement+reverse, row-splitting".into(),
    ]
}

fn metamorphic_check(seed: u64) {
    let mut rng = StdRng::seed_from_u64(seed);
    let s = gen_scenario_cfg(seed, &META_CFG);
    let members = &s.members;
    let pay = s.payments();
    let sh = s.shares();
    let set = s.settlements.clone();
    let base = net_balances(members, &pay, &sh, &set);
    let ctx = || format!("metamorphic seed={seed}");

    // (1) Order independence.
    let (mut p, mut q, mut e) = (pay.clone(), sh.clone(), set.clone());
    shuffle(&mut p, &mut rng);
    shuffle(&mut q, &mut rng);
    shuffle(&mut e, &mut rng);
    assert_eq!(net_balances(members, &p, &q, &e), base, "order-dependent! {}", ctx());

    // (2) Scaling by k must scale balances by k (bounded to stay under i64::MAX).
    let maxb = base.iter().map(|(_, b)| b.abs()).max().unwrap_or(0);
    let kmax = if maxb == 0 { 1000 } else { (i64::MAX / (maxb + 1)).min(1000) };
    if kmax >= 2 {
        let k = rng.random_range(2..=kmax);
        let p: Vec<_> = pay.iter().map(|&(m, a)| (m, a * k)).collect();
        let q: Vec<_> = sh.iter().map(|&(m, a)| (m, a * k)).collect();
        let e: Vec<_> = set.iter().map(|&(f, t, a)| (f, t, a * k)).collect();
        let expect: Vec<_> = base.iter().map(|&(m, b)| (m, b * k)).collect();
        assert_eq!(net_balances(members, &p, &q, &e), expect, "not linear (k={k}) {}", ctx());
    }

    // (3) A self-paid expense changes nothing.
    let pid = members[rng.random_range(0..members.len())];
    let x = rng.random_range(1..=1_000_000i64);
    let (mut p, mut q) = (pay.clone(), sh.clone());
    p.push((pid, x));
    q.push((pid, x));
    assert_eq!(net_balances(members, &p, &q, &set), base, "self-paid expense moved money {}", ctx());

    // (4) A settlement and its reverse cancel.
    let a = members[rng.random_range(0..members.len())];
    let b = members[rng.random_range(0..members.len())];
    let m = rng.random_range(1..=1_000_000i64);
    let mut e = set.clone();
    e.push((a, b, m));
    e.push((b, a, m));
    assert_eq!(net_balances(members, &pay, &sh, &e), base, "settlement+reverse moved money {}", ctx());

    // (5) Splitting one payment row and one share row in two changes nothing.
    let mut p = pay.clone();
    if let Some(i) = p.iter().position(|&(_, a)| a >= 2) {
        let (m, a) = p[i];
        p[i] = (m, a / 2);
        p.push((m, a - a / 2));
    }
    let mut q = sh.clone();
    if let Some(i) = q.iter().position(|&(_, a)| a >= 2) {
        let (m, a) = q[i];
        q[i] = (m, a / 2);
        q.push((m, a - a / 2));
    }
    assert_eq!(net_balances(members, &p, &q, &set), base, "splitting a row moved money {}", ctx());
}

/// #4 Settle round-trip: record the suggested transfers as settlements, and the
/// group must land exactly square with nothing left to settle.
fn exp_roundtrip(deadline: Instant, threads: usize, base: u64) -> Vec<String> {
    let parts: Vec<(u64, usize)> = std::thread::scope(|sc| {
        (0..threads)
            .map(|t| {
                sc.spawn(move || {
                    let (mut count, mut max_t) = (0u64, 0usize);
                    let mut seed = base.wrapping_add(t as u64);
                    while Instant::now() < deadline {
                        let s = gen_scenario_cfg(seed, &RT_CFG);
                        let transfers = simplify(&s.balances());
                        let mut set = s.settlements.clone();
                        for tr in &transfers {
                            set.push((tr.from, tr.to, tr.amount));
                        }
                        let after = net_balances(&s.members, &s.payments(), &s.shares(), &set);
                        assert!(
                            after.iter().all(|&(_, b)| b == 0),
                            "settle round-trip left non-zero balances at seed={}\n{:#?}",
                            s.seed, s
                        );
                        assert!(simplify(&after).is_empty(), "residual transfers after settling at seed={}", s.seed);
                        count += 1;
                        max_t = max_t.max(transfers.len());
                        seed = seed.wrapping_add(threads as u64);
                    }
                    (count, max_t)
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect()
    });
    let count: u64 = parts.iter().map(|p| p.0).sum();
    let max_t = parts.iter().map(|p| p.1).max().unwrap_or(0);
    vec![format!("{count} groups settled to exactly zero (up to {max_t} transfers each), no residue")]
}

/// #8 Monthly-split lifecycle: add expenses, settle, repeat for many months —
/// the group must be exactly square after every settle, with no öre drift.
fn exp_lifecycle(deadline: Instant, threads: usize, base: u64) -> Vec<String> {
    #[derive(Default)]
    struct S { groups: u64, months: u64, max_months: u64, max_ledger: usize }
    let parts: Vec<S> = std::thread::scope(|sc| {
        (0..threads)
            .map(|t| {
                sc.spawn(move || {
                    let mut st = S::default();
                    let mut rng = StdRng::seed_from_u64(base ^ (t as u64).wrapping_mul(0xD1B5_4A32_D192_ED03));
                    while Instant::now() < deadline {
                        let n = rng.random_range(2..=8usize);
                        let mut members = Vec::with_capacity(n);
                        let mut id = rng.random_range(1..50i64);
                        for _ in 0..n {
                            members.push(id);
                            id += rng.random_range(1..5i64);
                        }
                        let (mut pay, mut sh, mut set) = (Vec::new(), Vec::new(), Vec::new());
                        let target = rng.random_range(50..=400u64);
                        let mut months = 0u64;
                        while months < target {
                            if Instant::now() >= deadline {
                                break;
                            }
                            for _ in 0..rng.random_range(1..=6) {
                                gen_expense_into(&mut rng, &members, 500_000, &mut pay, &mut sh);
                            }
                            let bal = net_balances(&members, &pay, &sh, &set);
                            for tr in simplify(&bal) {
                                set.push((tr.from, tr.to, tr.amount));
                            }
                            let after = net_balances(&members, &pay, &sh, &set);
                            assert!(
                                after.iter().all(|&(_, b)| b == 0),
                                "lifecycle drift: month {months}, thread {t}, {n} members"
                            );
                            months += 1;
                            st.months += 1;
                            st.max_ledger = st.max_ledger.max(pay.len() + set.len());
                            if pay.len() > 4000 {
                                break;
                            }
                        }
                        st.groups += 1;
                        st.max_months = st.max_months.max(months);
                    }
                    st
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect()
    });
    vec![format!(
        "{} groups over {} total months (up to {} months, {} ledger rows) — square after every settle",
        parts.iter().map(|p| p.groups).sum::<u64>(),
        parts.iter().map(|p| p.months).sum::<u64>(),
        parts.iter().map(|p| p.max_months).max().unwrap_or(0),
        parts.iter().map(|p| p.max_ledger).max().unwrap_or(0),
    )]
}

/// #2 Overflow / precision envelope: how much money before the i64 balance wraps.
fn overflow_probe() -> Vec<String> {
    let a: i64 = 1_000_000_000_000_000; // 1e15 öre = 10 trillion SEK per expense
    let members = [1i64, 2i64];
    let n_break = (i64::MAX / a) as usize; // largest count that still fits
    let matches = |n: usize| -> bool {
        let p: Vec<(i64, i64)> = std::iter::repeat_n((1i64, a), n).collect();
        let s: Vec<(i64, i64)> = std::iter::repeat_n((2i64, a), n).collect();
        let b64 = net_balances(&members, &p, &s, &[]);
        let b128 = balances_i128(&members, &p, &s, &[]);
        b64.iter().all(|&(id, v)| v as i128 == b128[&id])
    };
    let ok_below = matches(n_break);
    let diverge_above = !matches(n_break + 1);
    assert!(ok_below, "i64 diverged from i128 below the theoretical ceiling");
    assert!(diverge_above, "expected i64 overflow past the ceiling was not detected");
    vec![
        "── overflow / precision envelope ──".into(),
        format!("i64 balance ceiling: {} öre = {} SEK", i64::MAX, i64::MAX / 100),
        format!(
            "a member can absorb {n_break} expenses of {} SEK and still compute exactly (i64 == i128): OK",
            a / 100
        ),
        format!("the {}th such expense overflows i64, caught by the i128 oracle: confirmed", n_break + 1),
        "real tabs top out in the millions of SEK — roughly 9 orders of magnitude of headroom.".into(),
    ]
}

/// The campaign: a time-boxed soak across all experiments. Ignored by default.
#[test]
#[ignore = "long-running soak; run explicitly, e.g. SETTLEUP_SIM_SECS=1200 cargo test --release sim_campaign -- --ignored --nocapture"]
fn sim_campaign() {
    use std::io::Write;
    let secs: u64 = std::env::var("SETTLEUP_SIM_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(1200);
    let threads: usize = std::env::var("SETTLEUP_SIM_THREADS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4));

    let start = Instant::now();
    println!("\n=== SettleUp simulation campaign — {secs}s budget, {threads} threads ===\n");
    let _ = std::io::stdout().flush();

    for l in overflow_probe() {
        println!("{l}");
    }
    println!();
    let _ = std::io::stdout().flush();

    let fillers: [(&str, u32, fn(Instant, usize, u64) -> Vec<String>); 6] = [
        ("config-sweep fuzz", 3, exp_fuzz),
        ("optimality oracle", 3, exp_oracle),
        ("adversarial worst-case", 2, exp_adversarial),
        ("metamorphic properties", 2, exp_metamorphic),
        ("settle round-trip", 1, exp_roundtrip),
        ("monthly-split lifecycle", 2, exp_lifecycle),
    ];
    let total_w: u32 = fillers.iter().map(|(_, w, _)| *w).sum();
    let budget = secs.saturating_sub(start.elapsed().as_secs());

    for (i, (name, w, f)) in fillers.iter().enumerate() {
        let slice = (budget * *w as u64 / total_w as u64).max(1);
        let deadline = Instant::now() + Duration::from_secs(slice);
        let seed_base = (i as u64 + 1).wrapping_mul(0x1_0000_0001) ^ 0x5EED_BEEF_0000;
        println!("── [{}/6] {name}  (~{slice}s) ──", i + 1);
        let _ = std::io::stdout().flush();
        let t0 = Instant::now();
        for l in f(deadline, threads, seed_base) {
            println!("     {l}");
        }
        println!("     (took {:?})\n", t0.elapsed());
        let _ = std::io::stdout().flush();
    }
    println!("=== campaign complete in {:?} ===", start.elapsed());
}

/// The optimality oracle must return the *true* minimum on a case where greedy
/// is provably suboptimal — otherwise a broken oracle would silently report
/// "greedy is always optimal". Run this before trusting the campaign's numbers.
#[test]
fn oracle_matches_known_gap() {
    let mut sum = vec![0i64; 4096];
    let mut dp = vec![0i32; 4096];

    // Greedy needs 4 transfers here; the optimum is 3 ({4,-1,-3} and {3,-3}).
    let bal = [4i64, 3, -1, -3, -3];
    assert_eq!(min_transfers_optimal(&bal, &mut sum, &mut dp), 3);
    assert_eq!(simplify(&[(1, 4), (2, 3), (3, -1), (4, -3), (5, -3)]).len(), 4);

    assert_eq!(min_transfers_optimal(&[], &mut sum, &mut dp), 0);
    assert_eq!(min_transfers_optimal(&[5, -5], &mut sum, &mut dp), 1);
    assert_eq!(min_transfers_optimal(&[2, 2, -2, -2], &mut sum, &mut dp), 2);
    assert_eq!(min_transfers_optimal(&[3, -1, -1, -1], &mut sum, &mut dp), 3);

    // Greedy must never beat the optimum on random small scenarios.
    let mut rng = StdRng::seed_from_u64(0x0071_9A2C_u64);
    for _ in 0..3_000 {
        let s = gen_scenario_cfg(rng.random(), &ORACLE_CFG);
        let bal = s.balances();
        let nz: Vec<i64> = bal.iter().map(|(_, b)| *b).filter(|&b| b != 0).collect();
        if nz.len() > 12 {
            continue;
        }
        assert!(simplify(&bal).len() >= min_transfers_optimal(&nz, &mut sum, &mut dp));
    }
}

/// A soft-deleted expense must vanish from balances entirely — the balance
/// query's JOIN has to drop *both* the payment and its shares. (The real delete
/// risk lives in the SQL, not the pure math, so this exercises the DB path.)
#[tokio::test]
async fn soft_deleted_expense_leaves_no_trace_in_balances() {
    use crate::{db, settle};

    let pool = db::memory_pool().await;

    sqlx::query("INSERT INTO groups (id, name) VALUES ('g', 'G')").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO members (group_id, name, token_hash) VALUES ('g','Alice','a'), ('g','Bob','b')")
        .execute(&pool).await.unwrap();
    let ids: Vec<i64> = sqlx::query_as::<_, (i64,)>("SELECT id FROM members WHERE group_id='g' ORDER BY id")
        .fetch_all(&pool).await.unwrap().into_iter().map(|(x,)| x).collect();
    let (alice, bob) = (ids[0], ids[1]);

    // Alice fronts 100, split 50/50.
    let eid: i64 = sqlx::query_scalar(
        "INSERT INTO expenses (group_id, payer_id, amount, description) VALUES ('g', ?, 100, 'round') RETURNING id",
    ).bind(alice).fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO expense_shares (expense_id, member_id, amount) VALUES (?,?,50),(?,?,50)")
        .bind(eid).bind(alice).bind(eid).bind(bob).execute(&pool).await.unwrap();

    let members = vec![alice, bob];
    let pay = db::expense_payments(&pool, "g").await.unwrap();
    let sh = db::expense_share_rows(&pool, "g").await.unwrap();
    let set = db::settlement_rows(&pool, "g").await.unwrap();
    assert_eq!(settle::net_balances(&members, &pay, &sh, &set), vec![(alice, 50), (bob, -50)]);

    sqlx::query("UPDATE expenses SET deleted_at = datetime('now') WHERE id = ?")
        .bind(eid).execute(&pool).await.unwrap();

    let pay = db::expense_payments(&pool, "g").await.unwrap();
    let sh = db::expense_share_rows(&pool, "g").await.unwrap();
    let set = db::settlement_rows(&pool, "g").await.unwrap();
    assert_eq!(
        settle::net_balances(&members, &pay, &sh, &set),
        vec![(alice, 0), (bob, 0)],
        "soft-deleted expense still affects balances — its shares aren't excluded by the JOIN"
    );
}
