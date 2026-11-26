//! ゲームを起動せずに、独立した環境で Rin のテストを実行する。

use crate::ast::{Expr, ExprKind, Pattern, children};
use crate::parser::{OwnedAst, parse_owned, parse_tests};
use crate::{Builder, Error, HeapAllocator, Limits, Span, Value};
use std::collections::{BTreeMap, BTreeSet};

/// 失敗した検査の位置と評価済みの値。
#[derive(Debug)]
pub struct CheckFailure {
    pub at: Span,
    pub actual: Value,
    pub expected: Value,
    pub equality: bool,
}

/// テストの最終結果。実行エラーは検査失敗より優先する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestStatus {
    Passed,
    Failed,
    Error,
}

/// 1つのテストの結果。エラーの前に起きた検査失敗も保持する。
#[derive(Debug)]
pub struct TestResult {
    pub source: String,
    pub name: String,
    pub at: Span,
    pub status: TestStatus,
    pub failures: Vec<CheckFailure>,
    pub error: Option<Error>,
}

/// テストを実行する前の構文・モジュール検査エラー。
#[derive(Debug)]
pub struct SourceError {
    pub source: String,
    pub error: Error,
}

/// 全テストの結果。
#[derive(Debug, Default)]
pub struct TestReport {
    pub tests: Vec<TestResult>,
    pub errors: Vec<SourceError>,
}

impl TestReport {
    pub fn is_success(&self) -> bool {
        self.errors.is_empty() && self.tests.iter().all(|t| t.status == TestStatus::Passed)
    }
}

/// `(名前, ソース)` の列からテストを実行する。モジュールは import の依存順に読み込む。
/// テストファイルごとの定義・import は各テストの直前に新しい環境で評価する。
pub fn run(sources: &[(&str, &str)], modules: &[(&str, &str)], limits: Limits) -> TestReport {
    run_filtered(sources, modules, limits, None)
}

/// 名前に `filter` を含むテストのみ実行する。ソースの構文検査は全件に行う。
pub fn run_filtered(
    sources: &[(&str, &str)],
    modules: &[(&str, &str)],
    limits: Limits,
    filter: Option<&str>,
) -> TestReport {
    let mut report = TestReport::default();
    let order = match module_order(modules) {
        Ok(order) => order,
        Err(error) => {
            report.errors.push(*error);
            return report;
        }
    };
    for &(source_name, source) in sources {
        let program = match parse_tests(source).and_then(split_tests) {
            Ok(program) => program,
            Err(error) => {
                report.errors.push(SourceError {
                    source: source_name.into(),
                    error,
                });
                continue;
            }
        };
        for test in &program.tests {
            if filter.is_some_and(|pattern| !test.name.contains(pattern)) {
                continue;
            }
            let mut alloc = HeapAllocator;
            let mut machine = Builder::new(limits, &mut alloc);
            let mut error = None;
            for &idx in &order {
                let (name, source) = modules[idx];
                if let Err(e) = machine.load_module(name, source) {
                    error = Some(e);
                    break;
                }
            }
            let failures = if error.is_none() {
                let (failures, result) = machine.run_test(&program.setup, &test.body);
                error = result.err();
                failures
            } else {
                Vec::new()
            };
            let status = if error.is_some() {
                TestStatus::Error
            } else if failures.is_empty() {
                TestStatus::Passed
            } else {
                TestStatus::Failed
            };
            report.tests.push(TestResult {
                source: source_name.into(),
                name: test.name.clone(),
                at: test.at,
                status,
                failures,
                error,
            });
        }
    }
    report
}

struct Test {
    name: String,
    at: Span,
    body: Vec<Expr>,
}
struct Program {
    _ast: OwnedAst,
    setup: Vec<Expr>,
    tests: Vec<Test>,
}

fn split_tests(ast: OwnedAst) -> Result<Program, Error> {
    let mut setup = Vec::new();
    let mut tests = Vec::new();
    let mut names = BTreeSet::new();
    let exprs = ast.exprs();
    let globals = exprs
        .iter()
        .filter(|e| {
            !matches!(&e.kind,
                ExprKind::Call { callee, .. } if matches!(&callee.kind, ExprKind::Ident(n) if n.as_str() == "deftest"))
        })
        .fold(0, |mask, e| mask | declarations(e));
    for &expr in exprs {
        if let ExprKind::Call { callee, args } = &expr.kind
            && matches!(&callee.kind, ExprKind::Ident(name) if name.as_str() == "deftest")
        {
            let args = args.as_slice();
            let Some(Expr {
                kind: ExprKind::Str(name),
                ..
            }) = args.first()
            else {
                return Err(Error::parse(
                    "test.deftest",
                    expr.span,
                    "deftest requires a string name",
                ));
            };
            let name_str = name.as_str().to_owned();
            if !names.insert(name_str.clone()) {
                return Err(Error::parse(
                    "test.deftest",
                    expr.span,
                    "duplicate test name",
                ));
            }
            let bound = args[1..]
                .iter()
                .fold(globals, |mask, e| mask | declarations(e));
            for body in &args[1..] {
                validate(body, true, bound)?;
            }
            tests.push(Test {
                name: name_str,
                at: expr.span,
                body: args[1..].to_vec(),
            });
        } else {
            validate(&expr, false, globals)?;
            setup.push(expr);
        }
    }
    Ok(Program {
        _ast: ast,
        setup,
        tests,
    })
}

// 特殊形式と同名の値を、字面だけで拒否しないための束縛集合。
fn test_name(name: &str) -> u8 {
    match name {
        "deftest" => 1,
        "check" => 2,
        "check_eq" => 4,
        _ => 0,
    }
}

fn declarations(root: &Expr) -> u8 {
    let mut mask = 0;
    let mut work = vec![root];
    while let Some(expr) = work.pop() {
        match &expr.kind {
            ExprKind::Def { name, .. } => mask |= test_name(name.as_str()),
            ExprKind::Defn(d) => mask |= test_name(d.name.as_str()),
            _ => {}
        }
        children(expr, &mut work);
    }
    mask
}

fn validate(root: &Expr, in_test: bool, bound: u8) -> Result<(), Error> {
    let mut work = vec![(root, bound)];
    while let Some((expr, bound)) = work.pop() {
        if let ExprKind::Call { callee, args } = &expr.kind
            && let ExprKind::Ident(name) = &callee.kind
        {
            let name = name.as_str();
            let args = args.as_slice();
            if name == "deftest" {
                return Err(Error::parse(
                    "test.deftest",
                    expr.span,
                    "deftest is only allowed at the top level",
                ));
            }
            if matches!(name, "check" | "check_eq") {
                if !in_test {
                    return Err(Error::parse(
                        "test.check",
                        expr.span,
                        "checks are only allowed inside deftest",
                    ));
                }
                if args.len() != if name == "check" { 1 } else { 2 } {
                    return Err(Error::parse(
                        "test.check",
                        expr.span,
                        format!(
                            "{name} requires {} argument(s), got {}",
                            if name == "check" { 1 } else { 2 },
                            args.len()
                        ),
                    ));
                }
                work.extend(args.iter().map(|e| (e, bound)));
                continue;
            }
        }
        if matches!(&expr.kind, ExprKind::Ident(name) if test_name(name.as_str()) != 0 && bound & test_name(name.as_str()) == 0)
        {
            return Err(Error::parse(
                "test.check",
                expr.span,
                "test forms cannot be used as values",
            ));
        }
        match &expr.kind {
            ExprKind::Let(l) => {
                let mut scope = bound;
                for (name, value) in l.bindings.as_slice() {
                    work.push((value, scope));
                    scope |= test_name(name.as_str());
                }
                work.extend(l.body.as_slice().iter().map(|e| (e, scope)));
            }
            ExprKind::Fn(f) => {
                let scope = f
                    .params
                    .as_slice()
                    .iter()
                    .fold(bound, |mask, name| mask | test_name(name.as_str()));
                work.extend(f.body.as_slice().iter().map(|e| (e, scope)));
            }
            ExprKind::Defn(d) => {
                let scope = d
                    .params
                    .as_slice()
                    .iter()
                    .fold(bound, |mask, name| mask | test_name(name.as_str()));
                work.extend(d.body.as_slice().iter().map(|e| (e, scope)));
            }
            ExprKind::Match { scrutinee, arms } => {
                work.push((scrutinee.as_ref(), bound));
                for arm in arms.as_slice() {
                    let name = match &arm.pattern {
                        Pattern::Var(name) => Some(name.as_str()),
                        Pattern::Some(name)
                        | Pattern::Ok(name)
                        | Pattern::Err(name)
                        | Pattern::Variant { binding: name, .. } => name.as_ref().map(|s| s.as_str()),
                        _ => None,
                    };
                    let scope = bound | name.map_or(0, test_name);
                    work.extend(arm.body.as_slice().iter().map(|e| (e, scope)));
                }
            }
            _ => {
                let mut nested = Vec::new();
                children(expr, &mut nested);
                work.extend(nested.into_iter().map(|e| (e, bound)));
            }
        }
    }
    Ok(())
}

fn module_order(modules: &[(&str, &str)]) -> Result<Vec<usize>, Box<SourceError>> {
    let mut names = BTreeMap::new();
    let mut dependencies = Vec::new();
    for (i, &(name, source)) in modules.iter().enumerate() {
        if names.insert(name, i).is_some() {
            return Err(Box::new(SourceError {
                source: name.into(),
                error: Error::parse("test.modules", Span::default(), "duplicate module name"),
            }));
        }
        let ast = parse_owned(source, false).map_err(|error| {
            Box::new(SourceError {
                source: name.into(),
                error,
            })
        })?;
        let mut work: Vec<_> = ast.exprs().iter().collect();
        let mut imports = Vec::new();
        while let Some(expr) = work.pop() {
            if let ExprKind::Import { name } = &expr.kind {
                imports.push((name.as_str().to_owned(), expr.span));
            }
            children(expr, &mut work);
        }
        dependencies.push(imports);
    }
    let mut order = Vec::new();
    let mut done = BTreeSet::new();
    while order.len() < modules.len() {
        let before = order.len();
        for (i, deps) in dependencies.iter().enumerate() {
            if done.contains(&i) {
                continue;
            }
            for (name, at) in deps {
                if !names.contains_key(name.as_str()) {
                    return Err(Box::new(SourceError {
                        source: modules[i].0.into(),
                        error: Error::parse(
                            "test.modules",
                            *at,
                            format!("missing module '{name}'"),
                        ),
                    }));
                }
            }
            if deps
                .iter()
                .all(|(name, _)| done.contains(&names[name.as_str()]))
            {
                done.insert(i);
                order.push(i);
            }
        }
        if order.len() == before {
            let i = (0..modules.len()).find(|i| !done.contains(i)).unwrap();
            return Err(Box::new(SourceError {
                source: modules[i].0.into(),
                error: Error::parse(
                    "test.modules",
                    dependencies[i][0].1,
                    "cyclic module imports",
                ),
            }));
        }
    }
    Ok(order)
}

pub(crate) fn validate_source(source: &str) -> Result<(), Error> {
    split_tests(parse_tests(source)?).map(|_| ())
}
