use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use rin::{
    ArenaAllocator, Builder, Error, ErrorKind, HeapAllocator, Limits, Value, eval, eval_with,
    eval_with_allocator,
};

#[path = "spec_cases/determinism.rs"]
mod determinism;

#[derive(Debug)]
struct SpecCase {
    id: String,
    rule_id: String,
    error_rule: Option<String>,
    error_message: Option<String>,
    rendered: Option<String>,
    modules: Vec<(String, String)>,
    setup: Option<String>,
    failing_setup: Option<String>,
    initial_bindings: Vec<(String, String)>,
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
            let id = heading.split_whitespace().next().unwrap_or("").to_string();
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
                if !in_fence
                    && (cur.starts_with("# ") || cur.starts_with("## ") || cur.starts_with("### "))
                {
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

// 主根拠・期待規則はともにバッククォート付きの `[rule.id]` とする。
fn parse_rule_reference(text: &str) -> Option<&str> {
    let id = text.trim().strip_prefix("`[")?.strip_suffix("]`")?;
    (!id.is_empty() && !id.chars().any(|c| c.is_whitespace() || "[]`".contains(c))).then_some(id)
}

#[test]
fn rule_references_require_code_and_brackets() {
    assert_eq!(parse_rule_reference("`[op.arith]`"), Some("op.arith"));
    assert_eq!(
        parse_rule_reference(" `[src.nesting-limit]` "),
        Some("src.nesting-limit")
    );
    for malformed in [
        "op.arith",
        "[op.arith]",
        "`op.arith`",
        "`[op.arith`",
        "`op.arith]`",
        "`[]`",
        "`[[op.arith]]`",
        "`[op. arith]`",
        "`[op.arith]` trailing",
    ] {
        assert_eq!(parse_rule_reference(malformed), None, "{malformed}");
    }
}

#[test]
fn parse_single_case_rejects_malformed_headings_and_preserves_commas_in_bindings() {
    // 初期束縛の値にカンマを含む文字列を書いても 1 件のまま保持する
    let valid = [
        "主根拠：`[env.isolation]`",
        "初期束縛：`s = \"a,b\"`",
        "初期束縛：`x = 1`",
        "**入力：**",
        "```rin",
        "s",
        "```",
        "**期待結果：**",
        "| 上限 | 結果 | 値 |",
        "|---|---|---|",
        "| 既定 | 値 | `\"a,b\"` |",
    ];
    let parsed = parse_single_case("eval-999".to_string(), &valid, "test");
    assert_eq!(
        parsed.initial_bindings,
        vec![
            ("s".to_string(), "\"a,b\"".to_string()),
            ("x".to_string(), "1".to_string()),
        ]
    );

    // 1. 見出しのあとにコードブロックがない
    let missing_block = [
        "主根拠：`[expr.def]`",
        "**読み込み：**",
        "**入力：**",
        "```rin",
        "1",
        "```",
        "**期待結果：**",
        "| 上限 | 結果 | 値 |",
        "|---|---|---|",
        "| 既定 | 値 | `1` |",
    ];
    assert!(
        std::panic::catch_unwind(|| parse_single_case("t-1".to_string(), &missing_block, "test"))
            .is_err()
    );

    // 2. 入力コードブロックが 2 つある
    let dup_input = [
        "主根拠：`[expr.def]`",
        "**入力：**",
        "```rin",
        "1",
        "```",
        "**入力：**",
        "```rin",
        "2",
        "```",
        "**期待結果：**",
        "| 上限 | 結果 | 値 |",
        "|---|---|---|",
        "| 既定 | 値 | `1` |",
    ];
    assert!(
        std::panic::catch_unwind(|| parse_single_case("t-2".to_string(), &dup_input, "test"))
            .is_err()
    );

    // 3. 未知の **...** 見出し（全角/半角コロンの書き間違い等）
    let unknown_heading = [
        "主根拠：`[expr.def]`",
        "**読み込み:**",
        "```rin",
        "(def x 1)",
        "```",
        "**入力：**",
        "```rin",
        "x",
        "```",
        "**期待結果：**",
        "| 上限 | 結果 | 値 |",
        "|---|---|---|",
        "| 既定 | 値 | `1` |",
    ];
    assert!(
        std::panic::catch_unwind(|| parse_single_case("t-3".to_string(), &unknown_heading, "test"))
            .is_err()
    );
}

fn parse_single_case(id: String, lines: &[&str], file_label: &str) -> SpecCase {
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum PendingBlock {
        Input,
        Setup,
        FailingSetup,
        Module(String),
        ExpectedRender,
    }

    let mut rule_id = None;
    let mut error_rule = None;
    let mut error_message = None;
    let mut rendered = None;
    let mut modules = Vec::new();
    let mut setup = None;
    let mut failing_setup = None;
    let mut initial_bindings = Vec::new();
    let mut source = None;
    let mut table_row = None;
    let mut pending: Option<PendingBlock> = None;

    let mut idx = 0;
    while idx < lines.len() {
        let trimmed = lines[idx].trim();
        if let Some(rest) = trimmed.strip_prefix("主根拠：") {
            let rule = parse_rule_reference(rest)
                .unwrap_or_else(|| panic!("{file_label} ({id}): 主根拠 must use `[rule.id]`"));
            rule_id = Some(format!("[{rule}]"));
        } else if let Some(rest) = trimmed.strip_prefix("期待規則：") {
            let rule = parse_rule_reference(rest)
                .unwrap_or_else(|| panic!("{file_label} ({id}): 期待規則 must use `[rule.id]`"));
            error_rule = Some(rule.to_string());
        } else if let Some(rest) = trimmed.strip_prefix("期待メッセージ：") {
            error_message = Some(
                rest.trim()
                    .strip_prefix('`')
                    .and_then(|s| s.strip_suffix('`'))
                    .unwrap_or_else(|| {
                        panic!("{file_label} ({id}): 期待メッセージ must use backticks")
                    })
                    .to_string(),
            );
        } else if let Some(rest) = trimmed.strip_prefix("初期束縛：") {
            let raw = rest
                .trim()
                .strip_prefix('`')
                .and_then(|s| s.strip_suffix('`'))
                .unwrap_or_else(|| panic!("{file_label} ({id}): 初期束縛 must use backticks"));
            let (name, expr) = raw.split_once('=').unwrap_or_else(|| {
                panic!("{file_label} ({id}): 初期束縛 entry must be `name = expr`")
            });
            let name = name.trim();
            assert!(
                !name.is_empty() && !name.chars().any(|c| c.is_whitespace() || c == ','),
                "{file_label} ({id}): invalid binding name '{name}' in 初期束縛 (use one `初期束縛：` line per binding)"
            );
            initial_bindings.push((name.to_string(), expr.trim().to_string()));
        } else if trimmed.starts_with("**") && trimmed.ends_with("**") && trimmed.len() > 4 {
            assert!(
                pending.is_none(),
                "{file_label} ({id}): missing code block after heading {:?} before '{trimmed}'",
                pending.unwrap()
            );
            if trimmed == "**入力：**" {
                pending = Some(PendingBlock::Input);
            } else if trimmed == "**読み込み：**" {
                pending = Some(PendingBlock::Setup);
            } else if trimmed == "**失敗する読み込み：**" {
                pending = Some(PendingBlock::FailingSetup);
            } else if let Some(rest) = trimmed.strip_prefix("**モジュール `")
                && let Some(mod_name) = rest.strip_suffix("`：**")
            {
                assert!(
                    !mod_name.is_empty(),
                    "{file_label} ({id}): empty module name in heading"
                );
                pending = Some(PendingBlock::Module(mod_name.to_string()));
            } else if trimmed == "**期待表示：**" {
                pending = Some(PendingBlock::ExpectedRender);
            } else if trimmed == "**期待結果：**" {
                // 直後に Markdown 表が続く（コードブロックは不要）。
            } else {
                panic!("{file_label} ({id}): unknown section heading '{trimmed}'");
            }
        } else if trimmed == "```text" {
            assert_eq!(
                pending.take(),
                Some(PendingBlock::ExpectedRender),
                "{file_label} ({id}): ```text block must follow **期待表示：**"
            );
            assert!(
                rendered.is_none(),
                "{file_label} ({id}): duplicate expected rendering"
            );
            idx += 1;
            let mut buf = Vec::new();
            while idx < lines.len() && lines[idx].trim() != "```" {
                buf.push(lines[idx]);
                idx += 1;
            }
            assert!(
                idx < lines.len(),
                "{file_label} ({id}): unclosed ```text block"
            );
            rendered = Some(buf.join("\n"));
        } else if trimmed == "```rin" {
            let section = pending.take().unwrap_or_else(|| {
                panic!("{file_label} ({id}): ```rin block without preceding section heading")
            });
            idx += 1;
            let mut buf = Vec::new();
            while idx < lines.len() && lines[idx].trim() != "```" {
                buf.push(lines[idx]);
                idx += 1;
            }
            assert!(
                idx < lines.len(),
                "{file_label} ({id}): unclosed ```rin block"
            );
            let block = buf.join("\n");
            match section {
                PendingBlock::Input => {
                    assert!(
                        source.is_none(),
                        "{file_label} ({id}): duplicate **入力：** block"
                    );
                    source = Some(block);
                }
                PendingBlock::Setup => {
                    assert!(
                        setup.is_none(),
                        "{file_label} ({id}): duplicate **読み込み：** block"
                    );
                    setup = Some(block);
                }
                PendingBlock::FailingSetup => {
                    assert!(
                        failing_setup.is_none(),
                        "{file_label} ({id}): duplicate **失敗する読み込み：** block"
                    );
                    failing_setup = Some(block);
                }
                PendingBlock::Module(name) => modules.push((name, block)),
                PendingBlock::ExpectedRender => {
                    panic!(
                        "{file_label} ({id}): **期待表示：** must be followed by ```text, not ```rin"
                    )
                }
            }
        } else if trimmed.starts_with("|") && !trimmed.contains("---") && !trimmed.contains("上限")
        {
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

    assert!(
        pending.is_none(),
        "{file_label} ({id}): missing code block after heading {:?}",
        pending.unwrap()
    );

    let (limit_spec, expected_kind, expected_value) =
        table_row.unwrap_or_else(|| panic!("{file_label} ({id}): missing expectation table row"));

    SpecCase {
        id,
        rule_id: rule_id.unwrap_or_else(|| panic!("{file_label}: missing 主根拠")),
        error_rule,
        error_message,
        rendered,
        modules,
        setup,
        failing_setup,
        initial_bindings,
        source: source.unwrap_or_else(|| panic!("{file_label}: missing ```rin source block")),
        limit_spec,
        expected_kind,
        expected_value,
    }
}

fn evaluate_case(case: &SpecCase) -> Result<Value, Error> {
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

    let needs_builder = !case.modules.is_empty()
        || case.setup.is_some()
        || case.failing_setup.is_some()
        || !case.initial_bindings.is_empty();

    if needs_builder {
        assert!(
            arena_bytes.is_none(),
            "{}: arena= is not supported with builder setup",
            case.id
        );
        let mut alloc = HeapAllocator;
        let mut builder = Builder::new(limits, &mut alloc);
        for (mod_name, mod_src) in &case.modules {
            builder
                .load_module(mod_name, mod_src)
                .unwrap_or_else(|e| panic!("{}: failed to load module '{mod_name}': {e}", case.id));
        }
        if let Some(setup) = &case.setup {
            builder
                .eval(setup)
                .unwrap_or_else(|e| panic!("{}: setup eval failed: {e}", case.id));
        }
        if let Some(failing) = &case.failing_setup {
            assert!(
                builder.eval(failing).is_err(),
                "{}: failing_setup unexpectedly succeeded",
                case.id
            );
        }
        if !case.initial_bindings.is_empty() {
            let mut machine = builder.freeze().expect("freeze");
            let evaluated: Vec<(String, Value)> = case
                .initial_bindings
                .iter()
                .map(|(k, expr)| {
                    let v = eval(expr).unwrap_or_else(|e| {
                        panic!("{}: failed to eval binding '{k} = {expr}': {e}", case.id)
                    });
                    (k.clone(), v)
                })
                .collect();
            let refs: Vec<(&str, Value)> = evaluated
                .iter()
                .map(|(k, v)| (k.as_str(), v.clone()))
                .collect();
            machine.eval_with(&case.source, &refs)
        } else {
            builder.eval(&case.source)
        }
    } else if let Some(cap) = arena_bytes {
        let mut backing = vec![0u8; cap];
        let mut arena = ArenaAllocator::new(&mut backing);
        eval_with_allocator(&case.source, &[], limits, &mut arena)
    } else {
        eval_with(&case.source, &[], limits)
    }
}

fn run_case(case: &SpecCase) {
    let result = evaluate_case(case);
    if let Err(err) = &result {
        if let Some(expected) = &case.rendered {
            assert_eq!(
                &err.render(&case.source),
                expected,
                "{}: rendered diagnostic mismatch",
                case.id
            );
        }
        if let Some(expected) = &case.error_message {
            assert_eq!(
                &err.message, expected,
                "{}: error message mismatch",
                case.id
            );
        }
        assert_eq!(
            Some(err.rule()),
            case.error_rule.as_deref(),
            "{}: error rule mismatch: {err}",
            case.id
        );
    }
    match case.expected_kind.as_str() {
        "値" => {
            let actual = result.unwrap_or_else(|e| panic!("{}: unexpected error: {e}", case.id));
            let expected = eval(&case.expected_value).unwrap_or_else(|e| {
                panic!(
                    "{}: failed to eval expected '{}': {e}",
                    case.id, case.expected_value
                )
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
    assert!(
        !index.is_empty(),
        "no category table found in docs/spec-cases.md"
    );

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
            if case.expected_kind == "値" {
                assert!(
                    case.error_rule.is_none()
                        && case.error_message.is_none()
                        && case.rendered.is_none(),
                    "{}: value case has error rule",
                    case.id
                );
            } else {
                let rule = case.error_rule.as_ref().expect("error case needs 期待規則");
                assert!(
                    rule_ids.contains(&format!("[{rule}]")),
                    "{}: unknown error rule {rule}",
                    case.id
                );
            }
            run_case(case);
        }
    }
}
