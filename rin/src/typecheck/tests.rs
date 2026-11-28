//! 静的型検査器と永続型環境 (`TypeEnv`) の単体テスト。

use crate::alloc::ArenaAllocator;
use crate::error::ErrorKind;
use crate::mem::{AllocCore, MemoryConfig, PermArena, WorkArena};
use crate::parser::parse_program_work;

use super::{CheckOptions, TypeEnv, check_program_work};

struct TestHarness {
    buf: Vec<u8>,
}

impl TestHarness {
    fn new(bytes: usize) -> Self {
        Self {
            buf: vec![0u8; bytes],
        }
    }

    fn run<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut TypeEnv, &mut PermArena, &mut WorkArena, &mut AllocCore<'_>) -> R,
    {
        let mut alloc = ArenaAllocator::new(&mut self.buf);
        let cfg = MemoryConfig::default();
        let mut core = AllocCore::new(&mut alloc, usize::MAX);
        let mut perm = PermArena::new(cfg.perm);
        let mut work = WorkArena::new(cfg.work);
        let mut env = TypeEnv::new();
        let res = f(&mut env, &mut perm, &mut work, &mut core);
        work.destroy(&mut core);
        perm.destroy(&mut core);
        res
    }
}

fn check_ok(src: &str) {
    let mut h = TestHarness::new(2 * 1024 * 1024);
    h.run(|env, perm, work, core| {
        let exprs = parse_program_work(src, work, core, false).expect("parse failed");
        let res = check_program_work(
            exprs.as_slice(),
            env,
            perm,
            work,
            core,
            CheckOptions::default(),
        );
        if res.is_err() {
            let err = core.error.as_ref();
            panic!(
                "expected Ok, got {:?} [{}]: {}",
                err.kind(),
                err.rule(),
                err.message()
            );
        }
    });
}

fn check_err(src: &str, expected_kind: ErrorKind, expected_rule: &str) {
    let mut h = TestHarness::new(2 * 1024 * 1024);
    h.run(|env, perm, work, core| {
        let exprs = parse_program_work(src, work, core, false).expect("parse failed");
        let res = check_program_work(
            exprs.as_slice(),
            env,
            perm,
            work,
            core,
            CheckOptions::default(),
        );
        assert!(res.is_err(), "expected error for:\n{src}");
        let err = core.error.as_ref();
        assert_eq!(
            err.kind(),
            expected_kind,
            "kind mismatch for:\n{src}\ngot: {:?} [{}] {}",
            err.kind(),
            err.rule(),
            err.message()
        );
        assert_eq!(
            err.rule(),
            expected_rule,
            "rule mismatch for:\n{src}\ngot: {:?} [{}] {}",
            err.kind(),
            err.rule(),
            err.message()
        );
    });
}

#[test]
fn basic_exprs_and_dead_branch_detection() {
    check_ok(
        r#"
(def x 42)
(def y f64 3.14)
(defn add ((a i64) (b i64)) -> i64
  (+ a b))
(add x 10)
"#,
    );

    // 実行されない分岐の型エラーも評価前に検出される
    check_err(r#"(if true 1 (+ 1 "oops"))"#, ErrorKind::Type, "op.arith");
    check_err(r#"(if true 1 "two")"#, ErrorKind::Type, "expr.if");
    check_err(r#"(if 1 2 3)"#, ErrorKind::Type, "expr.condition");
    // 未定義変数
    check_err(r#"(if false unknown_var 0)"#, ErrorKind::Type, "expr.name");
}

#[test]
fn def_inference_and_unresolved_error() {
    check_ok("(def xs (list i64) [])");
    check_ok("(def m (option text) none)");
    check_err("(def xs [])", ErrorKind::Type, "expr.def");
    check_err("(def m none)", ErrorKind::Type, "expr.def");
}

#[test]
fn bidirectional_higher_order_and_first_class_builtins() {
    check_ok(
        r#"
(defschema Card (struct Card (cost i64) (name text)))
(defn total-cost ((cards (list Card))) -> i64
  (fold (fn (acc c) (+ acc c.cost)) 0 cards))
(defn cheap-names ((cards (list Card))) -> (list text)
  (map (fn (c) c.name) (filter (fn (c) (< c.cost 3)) cards)))
(defn invert ((bs (list bool))) -> (list bool)
  (map not bs))
(defn sum ((ns (list i64))) -> i64
  (fold + 0 ns))
"#,
    );
    // 順序比較不可の型で sort / sort_by は型エラー
    check_err(
        "(defn bad ((bs (list bool))) -> (list bool) (sort bs))",
        ErrorKind::Type,
        "fn.list",
    );
}

#[test]
fn defschema_struct_open_variant_clone_and_mutual_recursion() {
    check_ok(
        r#"
(defschema Node (struct Node (val i64) (next (option Node))))
(defschema OpenCfg (open_struct OpenCfg (name text)))
(defschema Elem (variant Elem (Plain) (Fire i64)))
(defschema ElemCopy Elem)

(defn read-cfg ((c OpenCfg)) -> text c.name)
(defn fire-dmg ((e Elem)) -> i64
  (match e
    (Elem.Plain 0)
    (Elem.Fire(d) d)))
"#,
    );

    // CloneOf は別の固有型（Elem と ElemCopy は互換でない）
    check_err(
        r#"
(defschema Elem (variant Elem (Plain) (Fire)))
(defschema ElemCopy Elem)
(defn f ((e Elem)) -> ElemCopy e)
"#,
        ErrorKind::Type,
        "expr.defn",
    );

    // 循環 CloneOf は拒否
    check_err(
        r#"
(defschema A B)
(defschema B A)
"#,
        ErrorKind::Type,
        "type.defschema",
    );

    // open_struct の未宣言フィールド参照は型エラー
    check_err(
        r#"
(defschema OpenCfg (open_struct OpenCfg (name text)))
(defn bad ((c OpenCfg)) -> i64 c.extra)
"#,
        ErrorKind::Type,
        "expr.field",
    );

    // 構造体リテラルのスペルミス・欠落を検出
    check_err(
        r#"
(defschema HitResult (struct HitResult (dmg i64) (effect text)))
(defn bad () -> HitResult
  HitResult { dmg: 10 efect: "fire" })
"#,
        ErrorKind::Type,
        "expr.field",
    );
}

#[test]
fn datum_quote_quasiquote_coercion_and_match_exhaustiveness() {
    // Issue #5 の Suz マクロ利用例 + quote 規則 + == / != 対称性
    check_ok(
        r#"
(defn rewrite ((ast datum)) -> datum
  (match ast
    (list(xs)
     (match (car xs)
       (some(head)
        (if (== head 'old)
          `(new ,@(map rewrite (cdr xs)))
          `(,head ,@(map rewrite (cdr xs)))))
       (none ast)))
    (_ ast)))

(defn datum-all ((d datum)) -> i64
  (match d
    (i64(n) n)
    (f64(x) (as_i64 x))
    (bool(b) (if b 1 0))
    (char(_) 2)
    (text(s) (len s))
    (ident(id) (len (ident->text id)))
    (list(xs) (len xs))
    (none 0)))

(defn quote-types () -> bool
  (let (id 'foo
        kw 'def
        d1 '1
        d2 'true
        d3 'none
        ns [1 2 3])
    (and (== id 'foo)
         (== kw 'def)
         (== d1 1)
         (== 1 d1)
         (== d2 true)
         (== d3 none)
         (== none d3)
         (!= d1 'foo)
         (== `(a ,id ,@ns) `(a foo 1 2 3)))))
"#,
    );

    // 異型具体値同士の == は型エラー（(== 1 'x)）
    check_err("(== 1 'x)", ErrorKind::Type, "op.cmp");
    check_err("(== 'x 1)", ErrorKind::Type, "op.cmp");

    // datum の match 非網羅（none が欠落）
    check_err(
        r#"
(defn bad ((d datum)) -> i64
  (match d
    (i64(n) n)
    (f64(_) 0)
    (bool(_) 0)
    (char(_) 0)
    (text(_) 0)
    (ident(_) 0)
    (list(_) 0)))
"#,
        ErrorKind::Type,
        "expr.match",
    );

    // 到達不能枝の検出
    check_err(
        r#"
(defn bad ((b bool)) -> i64
  (match b
    (true 1)
    (false 0)
    (_ 2)))
"#,
        ErrorKind::Type,
        "expr.match",
    );
}

#[test]
fn modules_export_import_and_rollback() {
    let mut h = TestHarness::new(2 * 1024 * 1024);
    h.run(|env, perm, work, core| {
        // 1. game モジュールを読み込む
        let game_src = r#"
(defschema Element (variant Element (Plain) (Fire)))
(defschema HitCtx (struct HitCtx (atk i64) (element Element)))
(defn base-dmg ((c HitCtx)) -> i64 c.atk)
(defn secret () -> i64 99)
(export Element HitCtx base-dmg)
"#;
        let game_exprs = parse_program_work(game_src, work, core, false).unwrap();
        check_program_work(
            game_exprs.as_slice(),
            env,
            perm,
            work,
            core,
            CheckOptions {
                module_name: Some("game"),
                ..CheckOptions::default()
            },
        )
        .unwrap();

        // 2. import して公開型・公開関数を利用
        let mod_src = r#"
(import game)
(defn calc ((ctx game.HitCtx)) -> i64
  (match ctx.element
    (game.Element.Fire (* (game.base-dmg ctx) 2))
    (game.Element.Plain (game.base-dmg ctx))))
"#;
        let mod_exprs = parse_program_work(mod_src, work, core, false).unwrap();
        check_program_work(
            mod_exprs.as_slice(),
            env,
            perm,
            work,
            core,
            CheckOptions::default(),
        )
        .unwrap();

        // 3. 非公開シンボル参照は型エラー
        let bad_src = r#"
(import game)
(game.secret)
"#;
        let bad_exprs = parse_program_work(bad_src, work, core, false).unwrap();
        assert!(
            check_program_work(
                bad_exprs.as_slice(),
                env,
                perm,
                work,
                core,
                CheckOptions::default(),
            )
            .is_err()
        );
        assert_eq!(core.error.as_ref().rule(), "expr.module");

        // 4. スナップショットとロールバック
        let snap = env.snapshot();
        let tmp_src = "(def temp_val 123)";
        let tmp_exprs = parse_program_work(tmp_src, work, core, false).unwrap();
        check_program_work(
            tmp_exprs.as_slice(),
            env,
            perm,
            work,
            core,
            CheckOptions::default(),
        )
        .unwrap();
        assert!(env.lookup_root_val("temp_val").is_some());
        env.rollback(snap);
        assert!(env.lookup_root_val("temp_val").is_none());
    });
}

#[test]
fn template_and_test_mode() {
    check_ok(r#"(defn fmt-ok ((n i64) (id ident)) -> text "n=${n}, id=${id}")"#);
    check_err(
        r#"(defn fmt-bad ((xs (list i64))) -> text "xs=${xs}")"#,
        ErrorKind::Type,
        "expr.template",
    );

    let mut h = TestHarness::new(2 * 1024 * 1024);
    h.run(|env, perm, work, core| {
        let src = r#"
(deftest "math"
  (check (== (+ 1 2) 3))
  (check_eq (+ 1 2) 3))
"#;
        let exprs = parse_program_work(src, work, core, true).unwrap();
        check_program_work(
            exprs.as_slice(),
            env,
            perm,
            work,
            core,
            CheckOptions {
                test_mode: true,
                ..CheckOptions::default()
            },
        )
        .unwrap();

        // check_eq に異型を渡すと型エラー
        let bad = r#"(deftest "bad" (check_eq 1 "one"))"#;
        let bad_exprs = parse_program_work(bad, work, core, true).unwrap();
        assert!(
            check_program_work(
                bad_exprs.as_slice(),
                env,
                perm,
                work,
                core,
                CheckOptions {
                    test_mode: true,
                    ..CheckOptions::default()
                },
            )
            .is_err()
        );
    });
}

#[test]
fn frozen_env_extra_bindings_and_work_budget_limit() {
    use super::TY_I64;

    let mut h = TestHarness::new(2 * 1024 * 1024);
    h.run(|env, perm, work, core| {
        // 1. 読み込み段階でモジュールを登録して凍結
        let m_src = "(defn inc ((x i64)) -> i64 (+ x 1))";
        let m_exprs = parse_program_work(m_src, work, core, false).unwrap();
        check_program_work(
            m_exprs.as_slice(),
            env,
            perm,
            work,
            core,
            CheckOptions {
                module_name: Some("m"),
                ..CheckOptions::default()
            },
        )
        .unwrap();
        perm.freeze();

        // 2. 凍結環境では import なしで m.inc を呼び、extra_bindings を使える
        let eval_src = "(m.inc x)";
        let eval_exprs = parse_program_work(eval_src, work, core, false).unwrap();
        check_program_work(
            eval_exprs.as_slice(),
            env,
            perm,
            work,
            core,
            CheckOptions {
                frozen: true,
                commit: false,
                extra_bindings: &[("x", TY_I64)],
                ..CheckOptions::default()
            },
        )
        .unwrap();

        // 3. 凍結環境での defschema は env.frozen
        let bad_ds = "(defschema Foo (struct Foo (x i64)))";
        let bad_exprs = parse_program_work(bad_ds, work, core, false).unwrap();
        assert!(
            check_program_work(
                bad_exprs.as_slice(),
                env,
                perm,
                work,
                core,
                CheckOptions {
                    frozen: true,
                    commit: false,
                    ..CheckOptions::default()
                },
            )
            .is_err()
        );
        assert_eq!(core.error.as_ref().rule(), "env.frozen");

        // 4. 作業回数予算超過は ErrorKind::TooManySteps ([eval.limits])
        let mut wpool =
            super::work_types::WorkTypePool::new(work, core, 1, crate::Span::default()).unwrap();
        assert!(wpool.tick(core, crate::Span::default()).is_ok());
        assert!(wpool.tick(core, crate::Span::default()).is_err());
        assert_eq!(core.error.as_ref().kind(), ErrorKind::TooManySteps);
        assert_eq!(core.error.as_ref().rule(), "eval.limits");
    });
}
