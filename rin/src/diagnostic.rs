//! エラー時だけ候補を検索し、既存メッセージに修正の手がかりを付ける。
use std::collections::BTreeSet;
use std::fmt::Write;

/// 呼び出し側から名前順・重複なしで渡す。表示しない名前は保持しない。
pub(crate) fn available<'a>(
    mut message: String,
    label: &str,
    mut names: impl Iterator<Item = &'a str>,
) -> String {
    write!(message, "; {label}: ").unwrap();
    let mut shown = 0;
    for name in names.by_ref().take(16) {
        if shown > 0 {
            message.push_str(", ");
        }
        message.push_str(name);
        shown += 1;
    }
    if shown == 0 {
        message.push_str("(none)");
    }
    let remaining = names.count();
    if remaining > 0 {
        write!(message, ", ... and {remaining} more").unwrap();
    }
    message
}

pub(crate) fn suggest<'a>(
    mut message: String,
    name: &str,
    names: impl Iterator<Item = &'a str>,
) -> String {
    let query: Vec<char> = name.chars().collect();
    let limit = match query.len() {
        0..=2 => return message,
        3..=5 => 1,
        _ => 2,
    };
    // 最大 3 件だけ保持する。同距離は名前順、重複候補は 1 件とする。
    let mut best = BTreeSet::new();
    for candidate in names {
        if candidate == name {
            continue;
        }
        if let Some(distance) = distance(&query, candidate, limit) {
            best.insert((distance, candidate));
            if best.len() > 3 {
                best.pop_last();
            }
        }
    }
    if !best.is_empty() {
        message.push_str("; did you mean ");
        for (i, (_, candidate)) in best.into_iter().enumerate() {
            if i > 0 {
                message.push_str(", ");
            }
            write!(message, "'{candidate}'").unwrap();
        }
        message.push('?');
    }
    message
}

// Unicode scalar 値単位の Levenshtein 距離。閾値の帯だけを計算する。
// 長い識別子でも O(limit * length) とし、距離が閾値を超えたら打ち切る。
fn distance(query: &[char], candidate: &str, limit: usize) -> Option<usize> {
    let n = query.len();
    let m = candidate.chars().count();
    if n.abs_diff(m) > limit {
        return None;
    }
    let far = limit + 1;
    let mut prev = vec![far; n + 1];
    let mut next = vec![far; n + 1];
    for (j, cell) in prev.iter_mut().enumerate().take(limit + 1) {
        *cell = j;
    }
    for (index, ch) in candidate.chars().enumerate() {
        let i = index + 1;
        let start = i.saturating_sub(limit).max(1);
        let end = (i + limit).min(n);
        next[start - 1] = if start == 1 { i.min(far) } else { far };
        let mut minimum = next[start - 1];
        for j in start..=end {
            next[j] = (prev[j] + 1)
                .min(next[j - 1] + 1)
                .min(prev[j - 1] + usize::from(query[j - 1] != ch));
            minimum = minimum.min(next[j]);
        }
        if end < n {
            next[end + 1] = far;
        }
        if minimum > limit {
            return None;
        }
        std::mem::swap(&mut prev, &mut next);
    }
    (prev[n] <= limit).then_some(prev[n])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_distance_matches_full_distance() {
        fn full(a: &[char], b: &str) -> usize {
            let mut row: Vec<usize> = (0..=a.len()).collect();
            for (i, ch) in b.chars().enumerate() {
                let mut diagonal = row[0];
                row[0] = i + 1;
                for j in 1..=a.len() {
                    let old = row[j];
                    row[j] = (row[j] + 1)
                        .min(row[j - 1] + 1)
                        .min(diagonal + usize::from(a[j - 1] != ch));
                    diagonal = old;
                }
            }
            row[a.len()]
        }
        let mut words = vec![String::new()];
        for _ in 0..5 {
            let previous = words.clone();
            words.extend(
                previous
                    .iter()
                    .flat_map(|s| ['a', '猫'].map(|ch| format!("{s}{ch}"))),
            );
            words.sort();
            words.dedup();
        }
        for a in &words {
            let chars: Vec<_> = a.chars().collect();
            for b in &words {
                let expected = full(&chars, b);
                for limit in 0..=2 {
                    assert_eq!(
                        distance(&chars, b, limit),
                        (expected <= limit).then_some(expected),
                        "{a:?}, {b:?}, {limit}"
                    );
                }
            }
        }
        let long = "a".repeat(10000);
        assert_eq!(
            distance(&long.chars().collect::<Vec<_>>(), &(long.clone() + "b"), 2),
            Some(1)
        );
    }

    #[test]
    fn suggestions_have_stable_order_and_limits() {
        assert_eq!(
            suggest(
                "error".into(),
                "cat",
                ["hat", "bat", "cut", "bat", "cap"].into_iter()
            ),
            "error; did you mean 'bat', 'cap', 'cut'?"
        );
        assert_eq!(suggest("error".into(), "ab", ["abc"].into_iter()), "error");
        assert_eq!(
            suggest("error".into(), "cat", ["xxcat", "cat"].into_iter()),
            "error"
        );
        assert_eq!(
            suggest(
                "error".into(),
                "damage",
                ["damages", "xxdamage", "damag"].into_iter()
            ),
            "error; did you mean 'damag', 'damages', 'xxdamage'?"
        );
    }
}
