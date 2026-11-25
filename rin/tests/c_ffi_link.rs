use std::path::Path;
use std::process::Command;

#[test]
fn c_program_links_librin_and_runs_e2e() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap();
    let target_dir = workspace_root.join("target/c_ffi_link_build");

    // Cargo のロックファイル競合を避けるため、独立した target-dir で staticlib をビルドする。
    let status = Command::new(env!("CARGO"))
        .args(["build", "--manifest-path"])
        .arg(manifest_dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&target_dir)
        .status()
        .expect("failed to invoke cargo build");
    assert!(status.success(), "cargo build for staticlib failed");

    let lib_dir = target_dir.join("debug");
    let include_dir = manifest_dir.join("include");
    let c_src = target_dir.join("host_test.c");
    let exe = target_dir.join("host_test");

    std::fs::write(
        &c_src,
        r#"
#include <assert.h>
#include <stdint.h>
#include <string.h>
#include "rin.h"

typedef struct {
    uint8_t buf[65536];
    size_t offset;
} Bump;

static uint8_t *bump_alloc(void *ctx, size_t size, size_t align) {
    Bump *b = (Bump *)ctx;
    if (size == 0) return b->buf;
    size_t aligned = (b->offset + align - 1) & ~(align - 1);
    if (aligned + size > sizeof(b->buf)) return NULL;
    b->offset = aligned + size;
    return b->buf + aligned;
}

int main(void) {
    Bump bump;
    bump.offset = 0;
    RinAllocator alloc = { &bump, bump_alloc, NULL };
    Rin *m = rin_new(rin_limits_default(), alloc);
    assert(m != NULL);

    RinEvalResult lr = rin_load(m, "base", "(defn make_card (c) Card { id: \"ember\", cost: (- c 1) })");
    assert(lr.ok && lr.value == NULL);
    assert(rin_last_error(m) == NULL);
    rin_res_free(&lr);

    RinValue *arg = rin_int(4);
    const RinValue *args[1] = { arg };
    RinEvalResult cr = rin_call_in(m, "base", "make_card", args, 1);
    rin_drop(arg);
    assert(cr.ok);
    assert(rin_check_str(cr.value, "(struct Card (id string) (cost i64))"));

    RinValue *cost_v = rin_field(cr.value, "cost");
    int64_t cost = 0;
    assert(rin_as_int(cost_v, &cost));
    assert(cost == 3);
    rin_drop(cost_v);

    rin_res_free(&cr);
    RinEvalResult failed = rin_load(m, "broken", "(defn fail () (/ 1 0)) (fail)");
    assert(!failed.ok && failed.value == NULL);
    const RinError *error = rin_last_error(m);
    assert(error != NULL && error->kind == RIN_ERR_RUNTIME);
    assert(strcmp(error->module, "broken") == 0);
    assert(strcmp(error->rule, "op.arith") == 0 && error->line == 1 && error->col > 0);
    assert(strstr(error->message, "division by zero") != NULL);
    assert(error->backtrace != NULL && strstr(error->backtrace, "fail") != NULL);
    assert(strstr(error->text, "broken") != NULL);
    assert(strstr(error->text, "[op.arith]\n") != NULL);
    rin_res_free(&failed);
    assert(rin_last_error(m) == error);
    assert(strstr(error->text, "division by zero") != NULL);

    assert(rin_freeze(m));
    failed = rin_eval(m, NULL);
    assert(!failed.ok);
    error = rin_last_error(m);
    assert(error->kind == RIN_ERR_HOST);
    assert(error->module == NULL && strcmp(error->rule, "host.argument") == 0 && error->backtrace == NULL);
    assert(error->line == 0 && error->col == 0);
    assert(error->message != NULL && error->text != NULL);
    rin_res_free(&failed);
    RinEvalResult success = rin_eval(m, "42");
    assert(success.ok && success.value != NULL && rin_last_error(m) == NULL);
    rin_res_free(&success);
    RinValue *bx = rin_int(10);
    RinValue *by = rin_int(32);
    RinBinding binds[2] = { { "x", bx }, { "y", by } };
    RinEvalResult with_res = rin_eval_with(m, "(+ x y)", binds, 2);
    rin_drop(bx);
    rin_drop(by);
    assert(with_res.ok && with_res.value != NULL && rin_last_error(m) == NULL);
    int64_t sum = 0;
    assert(rin_as_int(with_res.value, &sum) && sum == 42);
    rin_res_free(&with_res);
    assert(rin_last_error(NULL) == NULL);
    rin_free(m);
    assert(bump.offset > 0);
    return 0;
}
"#,
    )
    .expect("failed to write C test source");

    let mut cc = Command::new("cc");
    cc.arg(&c_src)
        .arg("-I")
        .arg(&include_dir)
        .arg(lib_dir.join("librin.a"))
        .arg("-o")
        .arg(&exe);
    if cfg!(target_os = "macos") {
        cc.args(["-framework", "Security", "-liconv"]);
    } else {
        cc.args(["-lpthread", "-ldl", "-lm"]);
    }
    let cc_status = cc.status().expect("failed to invoke C compiler");
    assert!(cc_status.success(), "C link failed");

    let run_status = Command::new(&exe)
        .status()
        .expect("failed to run linked C test binary");
    assert!(run_status.success(), "linked C test binary failed");
}
