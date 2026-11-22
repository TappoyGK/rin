use clap::{Parser, Subcommand, ValueEnum};
use rin::test::{TestReport, TestStatus};
use rin::{Error, Limits};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "rin", version, about = "Rin development tools")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Discover and run *_test.rin files.
    Test {
        paths: Vec<PathBuf>,
        #[arg(long, value_name = "DIR")]
        module_path: Vec<PathBuf>,
        /// Run tests whose names contain PATTERN (case-sensitive).
        #[arg(long, value_name = "PATTERN")]
        filter: Option<String>,
        #[arg(long, default_value_t = Limits::default().steps)]
        steps: usize,
        #[arg(long, default_value_t = Limits::default().depth)]
        depth: usize,
        #[arg(long, default_value_t = Limits::default().memory_bytes)]
        memory: usize,
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Json,
}

fn main() -> ExitCode {
    let Cli {
        command:
            Command::Test {
                mut paths,
                module_path,
                filter,
                steps,
                depth,
                memory,
                format,
            },
    } = Cli::parse();
    if paths.is_empty() {
        paths.push(PathBuf::from("."));
    }
    match execute(
        &paths,
        &module_path,
        filter.as_deref(),
        Limits {
            steps,
            depth,
            memory_bytes: memory,
        },
        format,
    ) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(error) => {
            match format {
                Format::Text => eprintln!("error: {error}"),
                Format::Json => println!(
                    "{}",
                    json!({"success": false, "tests": [], "errors": [{"message": error}]})
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

fn error_json(error: &Error) -> Value {
    json!({"kind": format!("{:?}", error.kind), "rule": error.rule(), "message": error.message,
        "line": error.at.line, "column": error.at.col, "module": error.module(),
        "display": error.to_string(), "backtrace": error.backtrace().iter().map(|frame| json!({
            "module": frame.module, "name": frame.name, "caller_module": frame.caller_module,
            "line": frame.at.line, "column": frame.at.col,
            "display": frame.to_string()
        })).collect::<Vec<_>>()})
}

fn report_json(report: &TestReport) -> Value {
    json!({"success": report.is_success(),
        "tests": report.tests.iter().map(|test| json!({
            "source": test.source, "name": test.name, "line": test.at.line, "column": test.at.col,
            "status": status(test.status),
            "failures": test.failures.iter().map(|f| json!({
                "line": f.at.line, "column": f.at.col, "check": if f.equality { "check_eq" } else { "check" },
                "actual": f.actual.to_string(), "expected": f.expected.to_string()
            })).collect::<Vec<_>>(),
            "error": test.error.as_ref().map(error_json)
        })).collect::<Vec<_>>(),
        "errors": report.errors.iter().map(|e| json!({"source": e.source, "error": error_json(&e.error)})).collect::<Vec<_>>()})
}
