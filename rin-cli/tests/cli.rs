use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rin-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn write(&self, path: &str, source: &str) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, source).unwrap();
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_rin"))
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn discovery_filter_module_loading_and_json() {
    let f = Fixture::new();
    f.write(
        "nested/a_test.rin",
        "(import a) (deftest \"日本語 good\" (check_eq a.n 4)) (deftest \"bad\" (check false))",
    );
    f.write("ignored.rin", "invalid (");
    f.write("target/skip_test.rin", "invalid (");
    f.write("modules/a.rin", "(import b) (def n (+ b.n 1))");
    f.write("modules/b.rin", "(def n 3)");
    let out = f.run(&[
        "test",
        ".",
        "nested",
        "--module-path",
        "modules",
        "--filter",
        "good",
        "--format",
        "json",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.matches("\"status\":").count(), 1);
    assert!(text.contains("\"name\":\"日本語 good\""));
    let out = f.run(&["test", "--module-path", "modules", "--format", "json"]);
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("\"status\":\"FAIL\""));
    assert!(text.contains("\"actual\":\"false\""));
}

#[test]
fn limits_diagnostics_and_exit_codes() {
    let f = Fixture::new();
    f.write("a_test.rin", "(deftest \"fail\" (check false) (/ 1 0))");
    let out = f.run(&["test"]);
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("check failed"));
    assert!(text.contains('^'));
    assert!(text.contains("1 errors"));
    let out = f.run(&[
        "test", "--steps", "0", "--depth", "3", "--memory", "4096", "--format", "json",
    ]);
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("\"kind\":\"TooManySteps\""));
    let out = f.run(&["test", "missing.rin", "--format", "json"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .starts_with("{\"success\":false,")
    );
    assert_eq!(f.run(&["test", "--steps", "no"]).status.code(), Some(2));
}

#[test]
fn explicit_files_and_syntax_errors() {
    let f = Fixture::new();
    f.write("explicit.rin", "(deftest \"a\" (check true))");
    assert!(f.run(&["test", "explicit.rin"]).status.success());
    f.write("bad_test.rin", "(deftest \"bad\" (check))");
    let out = f.run(&["test", "--format", "json"]);
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("\"rule\":\"test.check\""));
}

#[test]
fn formatter_check_write_and_invalid_file_protection() {
    let f = Fixture::new();
    f.write("a.rin", "( + 1  2 )");
    f.write("nested/b_test.rin", "(deftest \"x\"(check true))");
    f.write("target/ignored.rin", "(");
    let out = f.run(&["fmt", "--check"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .contains("b_test.rin")
    );
    assert_eq!(fs::read_to_string(f.0.join("a.rin")).unwrap(), "( + 1  2 )");
    let out = f.run(&["fmt"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(fs::read_to_string(f.0.join("a.rin")).unwrap(), "(+ 1 2)\n");
    assert!(f.run(&["fmt", "--check"]).status.success());
    f.write("a.rin", "( + 1 2 )");
    f.write("z.rin", "(");
    let out = f.run(&["fmt"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(fs::read_to_string(f.0.join("a.rin")).unwrap(), "( + 1 2 )");
    assert_eq!(fs::read_to_string(f.0.join("z.rin")).unwrap(), "(");
}

#[test]
fn formatter_stdin_help_and_options() {
    use std::io::Write;
    use std::process::Stdio;
    let f = Fixture::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_rin"))
        .args(["fmt", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"[1  2]").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert_eq!(out.stdout, b"[1, 2]\n");
    assert!(f.run(&["fmt", "--help"]).status.success());
    for args in [
        vec!["fmt", "--check", "--check"],
        vec!["fmt", "--steps", "1"],
        vec!["fmt", "-", "a.rin"],
        vec!["fmt", "--check=true"],
    ] {
        assert_eq!(f.run(&args).status.code(), Some(2), "{args:?}");
    }
    f.write("--name.rin", "[1 2]");
    assert!(f.run(&["fmt", "--", "--name.rin"]).status.success());
}

#[cfg(unix)]
#[test]
fn formatter_preserves_permissions_and_skips_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::new();
    f.write("a.rin", "[1 2]");
    fs::set_permissions(f.0.join("a.rin"), fs::Permissions::from_mode(0o640)).unwrap();
    symlink(".", f.0.join("loop")).unwrap();
    assert!(f.run(&["fmt"]).status.success());
    assert_eq!(
        fs::metadata(f.0.join("a.rin"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o640
    );
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 2);
}

#[test]
fn formatter_explicit_paths_and_stdin_marker() {
    let f = Fixture::new();
    f.write("-", "[1 2]");
    f.write("source", "[3 4]");
    for args in [
        vec!["fmt", "--check", "./-"],
        vec!["fmt", "--check", "--", "-"],
    ] {
        let output = f.run(&args);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains('/'));
    }
    assert!(f.run(&["fmt", "--", "-", "source"]).status.success());
    assert_eq!(fs::read_to_string(f.0.join("-")).unwrap(), "[1, 2]\n");
    assert_eq!(fs::read_to_string(f.0.join("source")).unwrap(), "[3, 4]\n");
    assert_eq!(f.run(&["fmt", "-", "source"]).status.code(), Some(2));
    assert_eq!(f.run(&["fmt", "-", "-"]).status.code(), Some(2));
}

#[cfg(unix)]
#[test]
fn formatter_rejects_explicit_symlinks() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    f.write("real.rin", "[1 2]");
    symlink("real.rin", f.0.join("link.rin")).unwrap();
    symlink(".", f.0.join("directory")).unwrap();
    for path in ["link.rin", "directory"] {
        for check in [true, false] {
            let args = if check {
                vec!["fmt", "--check", path]
            } else {
                vec!["fmt", path]
            };
            let output = f.run(&args);
            assert_eq!(output.status.code(), Some(2), "{output:?}");
            assert!(String::from_utf8_lossy(&output.stderr).contains("symlink"));
        }
    }
    assert_eq!(fs::read_to_string(f.0.join("real.rin")).unwrap(), "[1 2]");
}
