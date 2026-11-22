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
        "test", "--steps", "0", "--depth", "3", "--memory", "1024", "--format", "json",
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
