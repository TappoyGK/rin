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

/// 2 文字以下には候補を出さないため、コンパイラも名前情報を記録しなくてよい。
pub(crate) fn needs_name_candidates(name: &str) -> bool {
    name.chars().take(3).count() == 3
}

pub(crate) fn suggest<'a>(
    mut message: String,
    name: &str,
    names: impl Iterator<Item = &'a str>,
) -> String {
    if !needs_name_candidates(name) {
        return message;
    }
    let query: Vec<char> = name.chars().collect();
    let limit = if query.len() <= 5 { 1 } else { 2 };
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
    debug_assert!(limit <= 2);
    let n = query.len();
    let max_chars = n.saturating_add(limit);
    // 1 scalar は 1〜4 バイト。Unicode でも誤って候補を捨てない下限・上限。
    if candidate.len() < n.saturating_sub(limit) || candidate.len() > max_chars.saturating_mul(4) {
        return None;
    }
    let far = limit + 1;
    let mut prev = [far; 5];
    let mut prev_start = 0;
    let mut prev_end = n.min(limit);
    for (j, cell) in prev.iter_mut().enumerate().take(prev_end + 1) {
        *cell = j;
    }
    // 数え上げと DP を同じ上限付き走査で行い、先頭不一致なら末尾を読まず終了する。
    for (index, ch) in candidate
        .chars()
        .take(max_chars.saturating_add(1))
        .enumerate()
    {
        let i = index + 1;
        if i > max_chars {
            return None;
        }
        let start = i.saturating_sub(limit);
        let end = (i + limit).min(n);
        let mut next = [far; 5];
        let get = |j: usize| {
            if (prev_start..=prev_end).contains(&j) {
                prev[j - prev_start]
            } else {
                far
            }
        };
        let mut minimum = far;
        for j in start..=end {
            let value = if j == 0 {
                i
            } else {
                let left = if j > start { next[j - start - 1] } else { far };
                (get(j) + 1)
                    .min(left + 1)
                    .min(get(j - 1) + usize::from(query[j - 1] != ch))
            };
            next[j - start] = value;
            minimum = minimum.min(value);
        }
        if minimum > limit {
            return None;
        }
        prev = next;
        prev_start = start;
        prev_end = end;
    }
    if !(prev_start..=prev_end).contains(&n) {
        return None;
    }
    (prev[n - prev_start] <= limit).then_some(prev[n - prev_start])
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
