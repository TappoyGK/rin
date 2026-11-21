use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use rin::{ArenaAllocator, ErrorKind, Limits, eval, eval_with, eval_with_allocator};

#[derive(Debug)]
struct SpecCase {
    id: String,
    rule_id: String,
    source: String,
    limit_spec: String,
    expected_kind: String,
    expected_value: String,
}

fn docs_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("docs")
}

fn collect_rule_ids(spec_md: &str) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for line in spec_md.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("**[")
            && let Some((id, _)) = rest.split_once("]**")
        {
            ids.insert(format!("[{id}]"));
        }
    }
    ids
}

fn parse_index_counts(index_md: &str) -> BTreeMap<String, (String, usize)> {
    let mut out = BTreeMap::new();
    for line in index_md.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('|') {
            continue;
        }
        let cols: Vec<&str> = trimmed
            .split('|')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if cols.len() == 4
            && let Ok(count) = cols[3].parse::<usize>()
        {
            let file = cols[1]
                .split('(')
                .nth(1)
                .and_then(|s| s.strip_suffix(')'))
                .unwrap_or(cols[1])
                .to_string();
            let prefix = cols[2].trim_matches('`').to_string();
            out.insert(prefix, (file, count));
        }
    }
    out
}

fn parse_spec_cases_file(content: &str, file_label: &str) -> Vec<SpecCase> {
    let lines: Vec<&str> = content.lines().collect();
    let mut cases = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].trim();
        if let Some(heading) = line.strip_prefix("### ") {
            let id = heading
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string();
            assert!(
                !id.is_empty(),
                "{file_label}:{}: missing case ID in heading",
                i + 1
            );

            i += 1;
            let mut section_lines = Vec::new();
            let mut in_fence = false;
            while i < lines.len() {
                let cur = lines[i];
                if cur.trim_start().starts_with("```") {
                    in_fence = !in_fence;
                }
                if !in_fence && (cur.starts_with("# ") || cur.starts_with("## ") || cur.starts_with("### ")) {
                    break;
                }
                section_lines.push(cur);
                i += 1;
            }
            cases.push(parse_single_case(id, &section_lines, file_label));
        } else {
            i += 1;
        }
    }

    cases
}

fn parse_single_case(id: String, lines: &[&str], file_label: &str) -> SpecCase {
    let mut rule_id = None;
    let mut source = None;
    let mut table_row = None;

    let mut idx = 0;
    while idx < lines.len() {
        let trimmed = lines[idx].trim();
        if let Some(rest) = trimmed.strip_prefix("主根拠：") {
            let cleaned = rest.trim().trim_end_matches('。').trim_matches('`');
            rule_id = Some(cleaned.to_string());
        } else if trimmed == "```rin" {
            idx += 1;
            let mut buf = Vec::new();
            while idx < lines.len() && lines[idx].trim() != "```" {
                buf.push(lines[idx]);
                idx += 1;
            }
            assert!(idx < lines.len(), "{file_label} ({id}): unclosed ```rin block");
            source = Some(buf.join("\n"));
        } else if trimmed.starts_with("|") && !trimmed.contains("---") && !trimmed.contains("上限") {
            let cols: Vec<&str> = trimmed
                .split('|')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            if cols.len() == 3 {
                table_row = Some((
                    cols[0].to_string(),
                    cols[1].to_string(),
                    cols[2].trim_matches('`').to_string(),
                ));
            }
        }
        idx += 1;
    }

    let (limit_spec, expected_kind, expected_value) =
        table_row.unwrap_or_else(|| panic!("{file_label} ({id}): missing expectation table row"));

    SpecCase {
        id,
        rule_id: rule_id.unwrap_or_else(|| panic!("{file_label}: missing 主根拠")),
        source: source.unwrap_or_else(|| panic!("{file_label}: missing ```rin source block")),
        limit_spec,
        expected_kind,
        expected_value,
    }
}

fn run_case(case: &SpecCase) {
    let mut limits = Limits::default();
    let mut arena_bytes = None;

    if case.limit_spec != "既定" {
        if let Some(n) = case.limit_spec.strip_prefix("steps=") {
            limits.steps = n.parse().expect("valid steps=");
        } else if let Some(n) = case.limit_spec.strip_prefix("depth=") {
            limits.depth = n.parse().expect("valid depth=");
        } else if let Some(n) = case.limit_spec.strip_prefix("arena=") {
            arena_bytes = Some(n.parse::<usize>().expect("valid arena="));
        } else {
            panic!("{}: unknown limit spec '{}'", case.id, case.limit_spec);
        }
    }

    let result = if let Some(cap) = arena_bytes {
        let mut backing = vec![0u8; cap];
        let mut arena = ArenaAllocator::new(&mut backing);
        eval_with_allocator(&case.source, &[], limits, &mut arena)
    } else {
        eval_with(&case.source, &[], limits)
    };

    match case.expected_kind.as_str() {
        "値" => {
            let actual = result.unwrap_or_else(|e| panic!("{}: unexpected error: {e}", case.id));
            let expected = eval(&case.expected_value).unwrap_or_else(|e| {
                panic!("{}: failed to eval expected '{}': {e}", case.id, case.expected_value)
            });
            assert_eq!(actual, expected, "{}: value mismatch", case.id);
        }
        "字句エラー" => {
            let err = result.unwrap_err();
            assert_eq!(err.kind, ErrorKind::Lex, "{}: {err}", case.id);
        }
        "構文エラー" => {
            let err = result.unwrap_err();
            assert_eq!(err.kind, ErrorKind::Parse, "{}: {err}", case.id);
        }
        "実行時エラー" => {
            let err = result.unwrap_err();
            assert_eq!(err.kind, ErrorKind::Runtime, "{}: {err}", case.id);
        }
        "ステップ超過" => {
            let err = result.unwrap_err();
            assert_eq!(err.kind, ErrorKind::TooManySteps, "{}: {err}", case.id);
        }
        "深さ超過" => {
            let err = result.unwrap_err();
            assert_eq!(err.kind, ErrorKind::TooDeep, "{}: {err}", case.id);
        }
        "メモリ超過" => {
            let err = result.unwrap_err();
            assert_eq!(err.kind, ErrorKind::TooMuchMemory, "{}: {err}", case.id);
        }
        other => panic!("{}: unknown expected kind '{other}'", case.id),
    }
}

#[test]
fn verify_all_spec_cases() {
    let docs = docs_dir();
    let spec_md = fs::read_to_string(docs.join("spec.md")).unwrap();
    let rule_ids = collect_rule_ids(&spec_md);
    assert!(!rule_ids.is_empty(), "no rule IDs found in docs/spec.md");

    let index_md = fs::read_to_string(docs.join("spec-cases.md")).unwrap();
    let index = parse_index_counts(&index_md);
    assert!(!index.is_empty(), "no category table found in docs/spec-cases.md");

    let mut seen_ids = BTreeSet::new();
    for (prefix, (rel_file, expected_count)) in &index {
        let path = docs.join(rel_file);
        let content = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        let cases = parse_spec_cases_file(&content, rel_file);
        assert_eq!(
            cases.len(),
            *expected_count,
            "case count mismatch for {rel_file} (expected {expected_count}, found {})",
            cases.len()
        );
        for case in &cases {
            assert!(
                case.id.starts_with(&format!("{prefix}-")),
                "{}: ID '{}' must start with '{prefix}-'",
                rel_file,
                case.id
            );
            assert!(
                seen_ids.insert(case.id.clone()),
                "duplicate spec case ID '{}'",
                case.id
            );
            assert!(
                rule_ids.contains(&case.rule_id),
                "{}: unknown rule ID '{}' in spec.md",
                case.id,
                case.rule_id
            );
            run_case(case);
        }
    }
}
