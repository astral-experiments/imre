//! CPython 3.14 weights UTF-8 byte edits, with a cheaper ASCII case change.

/// Offer the first lexicographically tied candidate within CPython's edit budget.
pub(super) fn closest<'a>(wrong: &str, mut names: Vec<&'a str>) -> Option<&'a str> {
    names.retain(|name| wrong.starts_with('_') || !name.starts_with('_'));
    names.sort_unstable();
    declaration_order(wrong, &names)
}

/// The function binder breaks equal edit distances by original parameter order.
pub(super) fn declaration_order<'a>(wrong: &str, names: &[&'a str]) -> Option<&'a str> {
    if names.len() >= 750 {
        return None;
    }
    let mut best = None;
    let mut best_cost = usize::MAX;
    for &name in names {
        if name == wrong {
            continue;
        }
        let budget = (wrong.len() + name.len() + 3) / 3;
        let cost = distance(wrong.as_bytes(), name.as_bytes());
        if cost <= budget && cost < best_cost {
            best = Some(name);
            best_cost = cost;
        }
    }
    best
}

/// Trim common affixes before applying the same bounded row size as `_suggestions`.
fn distance(mut a: &[u8], mut b: &[u8]) -> usize {
    while !a.is_empty() && !b.is_empty() && a[0] == b[0] {
        a = &a[1..];
        b = &b[1..];
    }
    while !a.is_empty() && !b.is_empty() && a.last() == b.last() {
        a = &a[..a.len() - 1];
        b = &b[..b.len() - 1];
    }
    if a.is_empty() || b.is_empty() {
        return 2 * (a.len() + b.len());
    }
    if a.len() > 40 || b.len() > 40 {
        return usize::MAX;
    }
    let mut row: [usize; 41] = std::array::from_fn(|i| 2 * i);
    for (i, x) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = 2 * (i + 1);
        for (j, y) in b.iter().enumerate() {
            let old = row[j + 1];
            let sub = if x == y {
                0
            } else if x.eq_ignore_ascii_case(y) {
                1
            } else {
                2
            };
            row[j + 1] = (diagonal + sub).min(row[j] + 2).min(old + 2);
            diagonal = old;
        }
    }
    row[b.len()]
}
