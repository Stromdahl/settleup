//! The settle-up math, kept pure (no DB, no I/O) so it can be unit-tested directly.
//!
//! Balance convention: a member's *net* is what the group owes them. Positive means
//! they are owed money (a creditor); negative means they owe money (a debtor). The
//! nets always sum to zero.

use std::collections::HashMap;

/// One suggested payment: `from` pays `to` the given amount (in öre).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer {
    pub from: i64,
    pub to: i64,
    pub amount: i64,
}

/// Split `total` öre equally across `members`, distributing any leftover öre one at a
/// time to the earliest members so the shares still sum exactly to `total`.
pub fn equal_shares(total: i64, members: &[i64]) -> Vec<(i64, i64)> {
    let n = members.len() as i64;
    if n == 0 {
        return Vec::new();
    }
    let base = total / n;
    let remainder = total - base * n;
    members
        .iter()
        .enumerate()
        .map(|(i, &m)| {
            let extra = if (i as i64) < remainder { 1 } else { 0 };
            (m, base + extra)
        })
        .collect()
}

/// Compute each member's net balance from the raw ledger.
///
/// - `member_ids`: everyone in the group (so members with no activity appear as 0).
/// - `expenses`: `(payer_id, amount)` — the payer fronted `amount`.
/// - `shares`: `(member_id, amount)` — this member's share of some expense.
/// - `settlements`: `(from_id, to_id, amount)` — `from` paid `to` back.
///
/// Returns `(member_id, net)` in the order of `member_ids`.
pub fn net_balances(
    member_ids: &[i64],
    expenses: &[(i64, i64)],
    shares: &[(i64, i64)],
    settlements: &[(i64, i64, i64)],
) -> Vec<(i64, i64)> {
    let mut bal: HashMap<i64, i64> = member_ids.iter().map(|&m| (m, 0)).collect();
    for &(payer, amount) in expenses {
        *bal.entry(payer).or_insert(0) += amount;
    }
    for &(member, amount) in shares {
        *bal.entry(member).or_insert(0) -= amount;
    }
    for &(from, to, amount) in settlements {
        // Paying down a debt moves the debtor up toward zero and the creditor down.
        *bal.entry(from).or_insert(0) += amount;
        *bal.entry(to).or_insert(0) -= amount;
    }
    member_ids
        .iter()
        .map(|&m| (m, bal.get(&m).copied().unwrap_or(0)))
        .collect()
}

/// Reduce net balances to a small set of transfers that settles everyone.
///
/// Greedy largest-debtor-pays-largest-creditor. This is not guaranteed to be the
/// theoretical minimum number of transfers (that problem is NP-hard), but it is
/// simple, correct (everyone ends at zero), and produces few transfers in practice.
pub fn simplify(balances: &[(i64, i64)]) -> Vec<Transfer> {
    let mut creditors: Vec<(i64, i64)> =
        balances.iter().copied().filter(|&(_, b)| b > 0).collect();
    let mut debtors: Vec<(i64, i64)> = balances
        .iter()
        .copied()
        .filter(|&(_, b)| b < 0)
        .map(|(id, b)| (id, -b))
        .collect();
    // Largest amounts first for a tidy result.
    creditors.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    debtors.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let mut transfers = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < debtors.len() && j < creditors.len() {
        let pay = debtors[i].1.min(creditors[j].1);
        if pay > 0 {
            transfers.push(Transfer {
                from: debtors[i].0,
                to: creditors[j].0,
                amount: pay,
            });
        }
        debtors[i].1 -= pay;
        creditors[j].1 -= pay;
        if debtors[i].1 == 0 {
            i += 1;
        }
        if creditors[j].1 == 0 {
            j += 1;
        }
    }
    transfers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_split_distributes_remainder() {
        // 100 öre across 3 -> 34, 33, 33 (sums to 100)
        let s = equal_shares(100, &[1, 2, 3]);
        assert_eq!(s, vec![(1, 34), (2, 33), (3, 33)]);
        assert_eq!(s.iter().map(|(_, a)| a).sum::<i64>(), 100);
    }

    #[test]
    fn equal_split_even() {
        assert_eq!(equal_shares(1000, &[7, 8]), vec![(7, 500), (8, 500)]);
    }

    #[test]
    fn balances_simple_equal_dinner() {
        // Member 1 pays 300 for a dinner split equally among 1,2,3.
        let members = [1, 2, 3];
        let expenses = [(1, 300)];
        let shares = equal_shares(300, &[1, 2, 3]);
        let bal = net_balances(&members, &expenses, &shares, &[]);
        // 1 paid 300, owes 100 share -> +200 ; 2 and 3 -> -100 each
        assert_eq!(bal, vec![(1, 200), (2, -100), (3, -100)]);
        assert_eq!(bal.iter().map(|(_, b)| b).sum::<i64>(), 0);
    }

    #[test]
    fn subset_split_excludes_nonparticipants() {
        // Round of drinks paid by 1, shared only by 1 and 2; member 3 not involved.
        let members = [1, 2, 3];
        let expenses = [(1, 200)];
        let shares = [(1, 100), (2, 100)];
        let bal = net_balances(&members, &expenses, &shares, &[]);
        assert_eq!(bal, vec![(1, 100), (2, -100), (3, 0)]);
    }

    #[test]
    fn settlement_zeroes_out() {
        let members = [1, 2];
        let expenses = [(1, 200)];
        let shares = [(1, 100), (2, 100)];
        // 2 pays 1 back the 100 they owe.
        let settlements = [(2, 1, 100)];
        let bal = net_balances(&members, &expenses, &shares, &settlements);
        assert_eq!(bal, vec![(1, 0), (2, 0)]);
    }

    #[test]
    fn simplify_produces_settling_transfers() {
        // 1 is owed 200, 2 and 3 each owe 100.
        let bal = vec![(1, 200), (2, -100), (3, -100)];
        let transfers = simplify(&bal);
        // Everyone should net to zero after applying transfers.
        let mut net: HashMap<i64, i64> = bal.iter().copied().collect();
        for t in &transfers {
            *net.get_mut(&t.from).unwrap() += t.amount;
            *net.get_mut(&t.to).unwrap() -= t.amount;
        }
        assert!(net.values().all(|&v| v == 0));
        // Two debtors, one creditor -> exactly two transfers.
        assert_eq!(transfers.len(), 2);
    }

    #[test]
    fn simplify_nets_across_the_group() {
        // A owes B, B owes C: should simplify so A pays C directly where possible.
        let bal = vec![(1, -50), (2, 0), (3, 50)];
        let transfers = simplify(&bal);
        assert_eq!(
            transfers,
            vec![Transfer {
                from: 1,
                to: 3,
                amount: 50
            }]
        );
    }

    #[test]
    fn already_settled_group_has_no_transfers() {
        let bal = vec![(1, 0), (2, 0)];
        assert!(simplify(&bal).is_empty());
    }
}
