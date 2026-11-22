mod arguments;
mod help;
mod json;

use arguments::{Action, Format};
use json::{Json, array, object, optional, string};
use rin::test::{TestReport, TestStatus};
use rin::{Error, Limits};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

fn main() -> ExitCode {
    let options = match arguments::parse(std::env::args_os().skip(1)) {
        Ok(Action::Help(test)) => {
            println!("{}", if test { help::TEST_USAGE } else { help::USAGE });
            return ExitCode::SUCCESS;
        }
        Ok(Action::Version) => {
            println!("rin {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Ok(Action::Execute(options)) => options,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };
    match execute(
        &options.paths,
        &options.module_paths,
        options.filter.as_deref(),
        options.limits,
        options.format,
    ) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(error) => {
            match options.format {
                Format::Text => eprintln!("error: {error}"),
                Format::Json => println!(
                    "{}",
                    object([
                        ("success", Json::Bool(false)),
                        ("tests", array(std::iter::empty())),
                        (
                            "errors",
                            array(std::iter::once(object([("message", string(error))])))
                        ),
                    ])
                ),
            }
            ExitCode::from(2)
        }
    }
}

// Canonical paths deduplicate overlapping roots. Do not follow directory symlinks.
fn discover(
    path: &Path,
    tests: bool,
    explicit: bool,
    files: &mut BTreeSet<PathBuf>,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))? {
            let entry = entry.map_err(|e| e.to_string())?;
            if matches!(entry.file_name().to_str(), Some(".git" | "target")) {
                continue;
            }
            discover(&entry.path(), tests, false, files)?;
        }
    } else if metadata.is_file() {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let accepted = if tests {
            (explicit && name.ends_with(".rin")) || name.ends_with("_test.rin")
        } else {
            name.ends_with(".rin") && !name.ends_with("_test.rin")
        };
        if accepted {
            files.insert(fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?);
        } else if explicit {
            return Err(format!("{}: expected a .rin source file", path.display()));
        }
    }
    Ok(())
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn execute(
    paths: &[PathBuf],
    module_paths: &[PathBuf],
    filter: Option<&str>,
    limits: Limits,
    format: Format,
) -> Result<bool, String> {
    let mut test_files = BTreeSet::new();
    for path in paths {
        discover(path, true, true, &mut test_files)?;
    }
    let sources: Vec<_> = test_files
        .iter()
        .map(|p| Ok((p.display().to_string(), read(p)?)))
        .collect::<Result<_, String>>()?;
    let mut module_files = BTreeSet::new();
    for path in module_paths {
        if !path.is_dir() {
            return Err(format!(
                "{}: --module-path requires a directory",
                path.display()
            ));
        }
        discover(path, false, true, &mut module_files)?;
    }
    let modules: Vec<_> = module_files
        .iter()
        .map(|p| {
            let name = p
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| format!("{}: invalid module name", p.display()))?;
            Ok((name.to_string(), read(p)?))
        })
        .collect::<Result<_, String>>()?;
    let source_refs: Vec<_> = sources
        .iter()
        .map(|(n, s)| (n.as_str(), s.as_str()))
        .collect();
    let module_refs: Vec<_> = modules
        .iter()
        .map(|(n, s)| (n.as_str(), s.as_str()))
        .collect();
    let report = rin::test::run_filtered(&source_refs, &module_refs, limits, filter);
    let resolve = |name: Option<&str>, source: &str| -> Option<&str> {
        let entries = if name.is_some() {
            &module_refs
        } else {
            &source_refs
        };
        let key = name.unwrap_or(source);
        entries
            .iter()
            .find(|(n, _)| *n == key)
            .map(|(_, text)| *text)
    };
    match format {
        Format::Json => println!("{}", report_json(&report)),
        Format::Text => {
            println!("running {} tests", report.tests.len());
            for test in &report.tests {
                println!("{} {} ({})", status(test.status), test.name, test.source);
                for failure in &test.failures {
                    let message = if failure.equality {
                        "check_eq failed"
                    } else {
                        "check failed"
                    };
                    let diagnostic = Error::runtime("test.check", failure.at, message);
                    println!(
                        "{}",
                        diagnostic.render(resolve(None, &test.source).unwrap_or(""))
                    );
                    println!(
                        "  actual:   {}\n  expected: {}",
                        failure.actual, failure.expected
                    );
                }
                if let Some(error) = &test.error {
                    println!("{}", error.render_with(|name| resolve(name, &test.source)));
                }
            }
            for error in &report.errors {
                println!(
                    "ERROR {}\n{}",
                    error.source,
                    error.error.render(
                        resolve(None, &error.source)
                            .or_else(|| resolve(Some(&error.source), ""))
                            .unwrap_or("")
                    )
                );
            }
            let passed = report
                .tests
                .iter()
                .filter(|t| t.status == TestStatus::Passed)
                .count();
            let failed = report
                .tests
                .iter()
                .filter(|t| t.status == TestStatus::Failed)
                .count();
            let errors = report
                .tests
                .iter()
                .filter(|t| t.status == TestStatus::Error)
                .count()
                + report.errors.len();
            println!("{passed} passed, {failed} failed, {errors} errors");
        }
    }
    Ok(report.is_success())
}

fn status(status: TestStatus) -> &'static str {
    match status {
        TestStatus::Passed => "ok",
        TestStatus::Failed => "FAIL",
        TestStatus::Error => "ERROR",
    }
}

fn error_json(error: &Error) -> Json {
    object([
        ("kind", string(format!("{:?}", error.kind))),
        ("rule", string(error.rule())),
        ("message", string(&error.message)),
        ("line", Json::Number(error.at.line)),
        ("column", Json::Number(error.at.col)),
        ("module", optional(error.module())),
        ("display", string(error)),
        (
            "backtrace",
            array(error.backtrace().iter().map(|frame| {
                object([
                    ("module", optional(frame.module.as_deref())),
                    ("name", optional(frame.name.as_deref())),
                    ("caller_module", optional(frame.caller_module.as_deref())),
                    ("line", Json::Number(frame.at.line)),
                    ("column", Json::Number(frame.at.col)),
                    ("display", string(frame)),
                ])
            })),
        ),
    ])
}

fn report_json(report: &TestReport) -> Json {
    object([
        ("success", Json::Bool(report.is_success())),
        (
            "tests",
            array(report.tests.iter().map(|test| {
                object([
                    ("source", string(&test.source)),
                    ("name", string(&test.name)),
                    ("line", Json::Number(test.at.line)),
                    ("column", Json::Number(test.at.col)),
                    ("status", string(status(test.status))),
                    (
                        "failures",
                        array(test.failures.iter().map(|f| {
                            object([
                                ("line", Json::Number(f.at.line)),
                                ("column", Json::Number(f.at.col)),
                                (
                                    "check",
                                    string(if f.equality { "check_eq" } else { "check" }),
                                ),
                                ("actual", string(&f.actual)),
                                ("expected", string(&f.expected)),
                            ])
                        })),
                    ),
                    (
                        "error",
                        test.error.as_ref().map(error_json).unwrap_or(Json::Null),
                    ),
                ])
            })),
        ),
        (
            "errors",
            array(report.errors.iter().map(|e| {
                object([
                    ("source", string(&e.source)),
                    ("error", error_json(&e.error)),
                ])
            })),
        ),
    ])
}
