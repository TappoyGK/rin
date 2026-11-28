//! エラー時だけ候補を検索し、既存メッセージに修正の手がかりを付ける（システムヒープ不使用）。
use std::fmt::Write;

/// 呼び出し側から名前順・重複なしで渡す。表示しない名前は保持しない。
pub(crate) fn write_available<'a>(
    out: &mut (impl Write + ?Sized),
    label: &str,
    mut names: impl Iterator<Item = &'a str>,
) {
    let _ = write!(out, "; {label}: ");
    let mut shown = 0;
    for name in names.by_ref().take(16) {
        if shown > 0 {
            let _ = out.write_str(", ");
        }
        let _ = out.write_str(name);
        shown += 1;
    }
    if shown == 0 {
        let _ = out.write_str("(none)");
    }
    let remaining = names.count();
    if remaining > 0 {
        let _ = write!(out, ", ... and {remaining} more");
    }
}

/// 2 文字以下には候補を出さないため、コンパイラも名前情報を記録しなくてよい。
pub(crate) fn needs_name_candidates(name: &str) -> bool {
    match name.len() {
        0..=2 => false,
        12.. => true, // 1 scalar は最大 4 バイトなので、少なくとも 3 文字ある。
        _ => name.chars().nth(2).is_some(),
    }
}

/// スタック上の固定配列だけで編集距離の上位 3 候補を選び、`out` へ直接書き出す（システムヒープ不使用）。
pub(crate) fn write_suggest<'a>(
    out: &mut (impl Write + ?Sized),
    name: &str,
    names: impl Iterator<Item = &'a str>,
) {
    if !needs_name_candidates(name) {
        return;
    }
    let mut query_buf = ['\0'; 128];
    let mut query_len = 0;
    for ch in name.chars() {
        if query_len >= query_buf.len() {
            return;
        }
        query_buf[query_len] = ch;
        query_len += 1;
    }
    let query = &query_buf[..query_len];
    let limit = if query.len() <= 5 { 1 } else { 2 };
    // 最大 3 件だけ保持する。同距離は名前順、重複候補は 1 件とする。
    let mut best: [(usize, &str); 3] = [(usize::MAX, ""); 3];
    let mut best_len = 0;
    for candidate in names {
        if candidate == name {
            continue;
        }
        let Some(dist) = distance(query, candidate, limit) else {
            continue;
        };
        let entry = (dist, candidate);
        if best[..best_len].contains(&entry) {
            continue;
        }
        let pos = best[..best_len]
            .iter()
            .position(|existing| entry < *existing)
            .unwrap_or(best_len);
        if pos >= best.len() {
            continue;
        }
        let new_len = (best_len + 1).min(best.len());
        for i in (pos + 1..new_len).rev() {
            best[i] = best[i - 1];
        }
        best[pos] = entry;
        best_len = new_len;
    }
    if best_len > 0 {
        let _ = out.write_str("; did you mean ");
        for (i, &(_, candidate)) in best[..best_len].iter().enumerate() {
            if i > 0 {
                let _ = out.write_str(", ");
            }
            let _ = write!(out, "'{candidate}'");
        }
        let _ = out.write_char('?');
    }
}

#[cfg(test)]
pub(crate) fn suggest<'a>(
    mut message: String,
    name: &str,
    names: impl Iterator<Item = &'a str>,
) -> String {
    write_suggest(&mut message, name, names);
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
    #[test]
    fn candidate_length_fast_path_preserves_unicode_boundaries() {
        for name in ["", "ab", "é", "猫猫", "😀😀"] {
            assert!(!super::needs_name_candidates(name), "{name}");
        }
        for name in ["abc", "猫猫猫", "😀😀a", "😀😀😀", "abcdefghijkl"] {
            assert!(super::needs_name_candidates(name), "{name}");
        }
    }

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
