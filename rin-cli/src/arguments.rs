//! Suzと同様に、OS文字列のパスを保ったままコマンドとオプションを検査する。
use rin::Limits;
use std::{collections::BTreeSet, ffi::OsString, path::PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Text,
    Json,
}
#[derive(Clone, Copy)]
pub enum Command {
    Test,
    Fmt,
}

pub enum Action {
    Help(Option<Command>),
    Version,
    Execute(Options),
    Format(FmtOptions),
}
pub struct Options {
    pub paths: Vec<PathBuf>,
    pub module_paths: Vec<PathBuf>,
    pub filter: Option<String>,
    pub limits: Limits,
    pub format: Format,
}

pub fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Action, String> {
    let first = args
        .next()
        .ok_or_else(|| format!("missing command\n{}", crate::help::USAGE))?;
    match first.to_str() {
        Some("--help" | "--version") => {
            if args.next().is_some() {
                return Err("information options must be used alone".into());
            }
            return Ok(if first == "--help" {
                Action::Help(None)
            } else {
                Action::Version
            });
        }
        Some("test") => {}
        Some("fmt") => return parse_format(args),
        _ => return Err(format!("unknown command: {}", first.display())),
    }
    let mut options = Options {
        paths: Vec::new(),
        module_paths: Vec::new(),
        filter: None,
        limits: Limits::default(),
        format: Format::Text,
    };
    let mut seen = BTreeSet::new();
    let mut paths_only = false;
    let mut help = false;
    while let Some(arg) = args.next() {
        if !paths_only {
            match arg.to_str() {
                Some("--") => {
                    paths_only = true;
                    continue;
                }
                Some("--help") if !help => {
                    help = true;
                    continue;
                }
                Some(
                    name @ ("--module-path" | "--filter" | "--steps" | "--depth" | "--memory"
                    | "--format"),
                ) => {
                    if name != "--module-path" && !seen.insert(name.to_string()) {
                        return Err(format!("duplicate {name} option"));
                    }
                    let value = args
                        .next()
                        .filter(|v| !v.is_empty())
                        .ok_or_else(|| format!("expected a value after {name}"))?;
                    if name == "--module-path" {
                        options.module_paths.push(value.into());
                        continue;
                    }
                    let text = value
                        .to_str()
                        .ok_or_else(|| format!("expected UTF-8 text after {name}"))?;
                    match name {
                        "--filter" => options.filter = Some(text.into()),
                        "--steps" => {
                            options.limits.steps =
                                count(text).ok_or("expected a nonnegative integer after --steps")?
                        }
                        "--depth" => {
                            options.limits.depth =
                                count(text).ok_or("expected a nonnegative integer after --depth")?
                        }
                        "--memory" => {
                            options.limits.memory_bytes = bytes(text)
                                .ok_or("expected a byte count or K/M/G suffix after --memory")?
                        }
                        "--format" => {
                            options.format = match text {
                                "text" => Format::Text,
                                "json" => Format::Json,
                                _ => return Err("expected text or json after --format".into()),
                            }
                        }
                        _ => unreachable!(),
                    }
                    continue;
                }
                _ => {}
            }
            if arg.as_encoded_bytes().starts_with(b"-") {
                return Err(format!("invalid or duplicate option: {}", arg.display()));
            }
        }
        if arg.is_empty() {
            return Err("expected a nonempty path".into());
        }
        options.paths.push(arg.into());
    }
    if help {
        return Ok(Action::Help(Some(Command::Test)));
    }
    if options.paths.is_empty() {
        options.paths.push(PathBuf::from("."));
    }
    Ok(Action::Execute(options))
}

pub struct FmtOptions {
    pub paths: Vec<PathBuf>,
    pub check: bool,
}
fn parse_format(mut args: impl Iterator<Item = OsString>) -> Result<Action, String> {
    let mut paths = Vec::new();
    let mut check = false;
    let mut help = false;
    let mut paths_only = false;
    for arg in args.by_ref() {
        if !paths_only {
            match arg.to_str() {
                Some("--") => {
                    paths_only = true;
                    continue;
                }
                Some("--help") if !help => {
                    help = true;
                    continue;
                }
                Some("--check") if !check => {
                    check = true;
                    continue;
                }
                Some("-") => {}
                _ if arg.as_encoded_bytes().starts_with(b"-") => {
                    return Err(format!("invalid or duplicate option: {}", arg.display()));
                }
                _ => {}
            }
        }
        if arg.is_empty() {
            return Err("expected a nonempty path".into());
        }
        paths.push(PathBuf::from(arg));
    }
    if help {
        return Ok(Action::Help(Some(Command::Fmt)));
    }
    if paths.iter().any(|p| p.as_os_str() == "-") && paths.len() != 1 {
        return Err("stdin '-' must be used without other paths".into());
    }
    if paths.is_empty() {
        paths.push(PathBuf::from("."));
    }
    Ok(Action::Format(FmtOptions { paths, check }))
}

fn count(value: &str) -> Option<usize> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}
fn bytes(value: &str) -> Option<usize> {
    let (digits, factor) = match value.as_bytes().last() {
        Some(b'K') => (&value[..value.len() - 1], 1024),
        Some(b'M') => (&value[..value.len() - 1], 1024 * 1024),
        Some(b'G') => (&value[..value.len() - 1], 1024 * 1024 * 1024),
        _ => (value, 1),
    };
    count(digits)?.checked_mul(factor)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse_args(args: &[&str]) -> Result<Action, String> {
        parse(args.iter().map(OsString::from))
    }
    #[test]
    fn rejects_invalid_duplicate_and_overflow_options() {
        for args in [
            vec![],
            vec!["unknown"],
            vec!["--version", "test"],
            vec!["test", "--steps"],
            vec!["test", "--steps", "-1"],
            vec!["test", "--depth", "+1"],
            vec!["test", "--memory", "1.5M"],
            vec!["test", "--memory", "18446744073709551615G"],
            vec!["test", "--format", "yaml"],
            vec!["test", "--format=json"],
            vec!["test", "--steps", "1", "--steps", "2"],
            vec!["test", "--help", "--help"],
            vec!["test", "--filter", ""],
            vec!["test", ""],
        ] {
            assert!(parse_args(&args).is_err(), "{args:?}");
        }
    }
    #[test]
    fn paths_repeated_modules_and_sizes() {
        let Action::Execute(o) = parse_args(&[
            "test",
            "--memory",
            "16M",
            "--module-path",
            "a",
            "--module-path",
            "b",
            "--",
            "--file.rin",
        ])
        .unwrap() else {
            panic!()
        };
        assert_eq!(o.limits.memory_bytes, 16 * 1024 * 1024);
        assert_eq!(o.paths, [PathBuf::from("--file.rin")]);
        assert_eq!(o.module_paths, [PathBuf::from("a"), PathBuf::from("b")]);
        assert_eq!(bytes("0"), Some(0));
        assert_eq!(bytes("2K"), Some(2048));
        assert_eq!(bytes("1G"), Some(1024 * 1024 * 1024));
        assert!(matches!(
            parse_args(&["test", "--help"]).unwrap(),
            Action::Help(Some(Command::Test))
        ));
    }
    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_are_preserved() {
        use std::os::unix::ffi::OsStringExt;
        let path = OsString::from_vec(vec![b'a', 0xff]);
        let Action::Execute(o) = parse([OsString::from("test"), path.clone()].into_iter()).unwrap()
        else {
            panic!()
        };
        assert_eq!(o.paths, [PathBuf::from(path)]);
    }
}
