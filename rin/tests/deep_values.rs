//! 値の深さによる abort を子プロセスと小さいスタックで検出する。
use rin::ffi::*;
use rin::value::ListNode;
use rin::{Builder, ErrorKind, HeapAllocator, Limits, Schema, Value};
use std::collections::BTreeMap;
use std::fmt::Write;
use std::rc::Rc;

fn nested(shape: usize, depth: usize, leaf: Value) -> Value {
    let mut v = leaf;
    for _ in 0..depth {
        v = match shape {
            0 => Value::list(vec![v]),
            1 => Value::some(v),
            2 => Value::tuple(vec![v]),
            3 => Value::array(vec![v]),
            4 => Value::structure("S", BTreeMap::from([("x".into(), v)])),
            5 => Value::variant("V", "Next", Some(v)),
            6 => Value::ok(v),
            7 => Value::err(v),
            _ => unreachable!(),
        };
    }
    v
}
struct Count(usize);
impl std::fmt::Write for Count {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0 += s.len();
        Ok(())
    }
}
struct Fail;
impl std::fmt::Write for Fail {
    fn write_str(&mut self, _: &str) -> std::fmt::Result {
        Err(std::fmt::Error)
    }
}

fn deep_roundtrips() {
    let mut alloc = HeapAllocator;
    let mut m = Builder::new(
        Limits {
            steps: 1_000_000, // 深い構造体の名前・フィールド比較も計上する。
            memory_bytes: 128 * 1024 * 1024,
            ..Limits::default()
        },
        &mut alloc,
    );
    for shape in 0..8 {
        let value = nested(shape, 20_000, Value::Int(7));
        let other = nested(shape, 20_000, Value::Int(7));
        let different = nested(shape, 20_000, Value::Int(8));
        assert!(value == other);
        assert!(value != different);
        let mut output = Count(0);
        write!(&mut output, "{value} {value:?} {value:#?}").unwrap();
        assert!(output.0 > 20_000);
        assert!(write!(&mut Fail, "{value}").is_err());
        assert!(write!(&mut Fail, "{value:?}").is_err());
        assert!(value.approx_bytes() >= 20_000);
        m.bind("a", &value).unwrap();
        m.bind("b", &other).unwrap();
        assert!(m.eval("(== a b)").unwrap() == Value::Bool(true));
        assert!(m.eval("a").unwrap() == value);
        m.bind("b", &different).unwrap();
        assert!(m.eval("(== a b)").unwrap() == Value::Bool(false));
        m.bind("a", &Value::Void).unwrap();
        m.bind("b", &Value::Void).unwrap();
        assert_eq!(m.live_bytes(), 0);
        let shared = value.clone();
        drop(value);
        assert!(shared == other);
        drop(shared);
        drop(other);
        drop(different);
        // 最後の強参照が Weak と共存していても安全に解放する。
        let root = Rc::new(nested(shape, 100_000, Value::Int(0)).into());
        let weak = Rc::downgrade(&root);
        drop(Value::Some(root));
        assert!(weak.upgrade().is_none());
    }
    let flat = ListNode::from_vec((0..100_000).map(Value::Int).collect());
    let shared_tail = flat.tail();
    let mut count = Count(0);
    write!(&mut count, "{flat:?}").unwrap();
    drop(flat);
    assert_eq!(shared_tail.len(), 99_999);
    drop(shared_tail);
    // 公開 ListNode を直接保持する場合の head 方向の解放。
    let node = ListNode::Cons {
        head: nested(0, 100_000, Value::None),
        tail: ListNode::empty(),
        len: 1,
    };
    drop(node);
    // 同じ子への参照を複数持つ DAG。片方の解放で最後の参照に変わる経路も辿る。
    let mut diamond = Value::None;
    for _ in 0..100_000 {
        diamond = Value::tuple(vec![diamond.clone(), diamond]);
    }
    drop(diamond);
    // コンテナ間で共有する DAG。Value を分解して取り出した所有データも直接落とす。
    let mut mixed = Value::None;
    for i in 0..100_000 {
        let pair = Value::tuple(vec![mixed.clone(), mixed]);
        mixed = nested(i % 8, 1, pair);
    }
    let Value::Array(items) = Value::array(vec![mixed]) else {
        unreachable!();
    };
    let weak = Rc::downgrade(&items);
    let owned = Rc::try_unwrap(items).unwrap();
    assert!(weak.upgrade().is_none());
    drop(owned);
    let mut shared_node = ListNode::empty();
    for _ in 0..100_000 {
        shared_node = ListNode::cons(Value::List(shared_node.clone()), shared_node);
    }
    drop(shared_node);
    // 実際の Mod ソースから、既定 Limits で深い値を作る経路。
    drop(rin::eval("(fold (fn (acc x) [acc]) [] (range 0 5000))").unwrap());
}

fn schemas_and_ffi() {
    let option = nested(1, 20_000, Value::None);
    let schema = Schema::parse("(defschema R (option R)) R").unwrap();
    schema.check(&option).unwrap();
    let bad = nested(1, 20_000, Value::Int(0));
    let err = schema.check(&bad).unwrap_err();
    assert!(err.message.ends_with("expected option, got i64"));
    assert!(err.message.starts_with("$.some.some"));
    let variant = nested(5, 20_000, Value::variant("V", "End", None));
    Schema::parse("(defschema R (variant V (Next R) (End))) R")
        .unwrap()
        .check(&variant)
        .unwrap();
    let list = nested(0, 20_000, Value::empty_list());
    Schema::parse("(defschema R (list R)) R")
        .unwrap()
        .check(&list)
        .unwrap();
    unsafe {
        let schema = rin_schema(c"(defschema R (option R)) R".as_ptr());
        assert!(!schema.is_null());
        let m = rin_new(
            rin_limits_default(),
            RinAllocator {
                ctx: std::ptr::null_mut(),
                alloc: None,
                dealloc: None,
            },
        );
        let mut value = rin_none(m);
        for _ in 0..20_000 {
            let next = rin_some(m, value);
            rin_drop(m, value);
            value = next;
        }
        assert!(rin_check(m, value, schema));
        assert!(rin_check_str(
            m,
            value,
            c"(defschema R (option R)) R".as_ptr()
        ));
        let wrapper = rin_some(m, value);
        rin_drop(m, value);
        let inner = rin_unwrap(m, wrapper);
        rin_drop(m, wrapper);
        assert!(rin_bind(m, c"x".as_ptr(), inner));
        rin_drop(m, inner);
        assert!(rin_freeze(m));
        let result = rin_eval(m, c"x".as_ptr());
        assert!(result.ok);
        assert!(rin_check(m, result.value, schema));
        let mut structure = rin_none(m);
        for _ in 0..2_000 {
            let st = rin_struct(m, c"S".as_ptr());
            assert!(rin_struct_set(m, st, c"x".as_ptr(), structure));
            rin_drop(m, structure);
            structure = st;
        }
        let field = rin_field(m, structure, c"x".as_ptr());
        assert!(!field.is_null());
        rin_drop(m, structure);
        let scalar = rin_int(m, 1);
        assert!(rin_struct_set(m, field, c"x".as_ptr(), scalar));
        rin_drop(m, scalar);
        rin_drop(m, field);
        let leaf = rin_i64s(m, [42].as_ptr(), 1);
        let variant = rin_variant(m, c"V".as_ptr(), c"Next".as_ptr(), leaf);
        rin_drop(m, leaf);
        let payload = rin_unwrap(m, variant);
        rin_drop(m, variant);
        let item = rin_tuple_get(m, payload, 0);
        rin_drop(m, payload);
        rin_drop(m, item);
        rin_schema_free(schema);
        rin_free(m);
    }
}

fn failure_cleanup() {
    for shape in 0..8 {
        let good = nested(shape, 500, Value::Int(1));
        let bad = Value::tuple(vec![
            good.clone(),
            nested(shape, 500, Value::Builtin("not-a-builtin")),
        ]);
        let mut alloc = HeapAllocator;
        let mut m = Builder::new(Limits::default(), &mut alloc);
        m.bind("keep", &Value::str("retained")).unwrap();
        let before = m.live_bytes();
        for _ in 0..3 {
            assert_eq!(m.bind("bad", &bad).unwrap_err().kind, ErrorKind::Runtime);
            assert_eq!(m.live_bytes(), before);
            assert!(m.eval("keep").unwrap() == Value::str("retained"));
        }
        // メモリ上限を変えて、子の変換途中・親の確保失敗を通す。
        for budget in [0, 40, 100, 500, 4000, 8000] {
            let mut alloc = HeapAllocator;
            let mut m = Builder::new(
                Limits {
                    memory_bytes: budget,
                    ..Limits::default()
                },
                &mut alloc,
            );
            assert_eq!(
                m.bind("x", &good).unwrap_err().kind,
                ErrorKind::TooMuchMemory
            );
            assert_eq!(m.live_bytes(), 0);
            if budget >= 4000 {
                assert!(m.eval("42").unwrap() == Value::Int(42));
            }
        }
    }
}

fn allocator_failures() {
    use rin::Allocator;
    struct FailAfter {
        remaining: usize,
    }
    // SAFETY: 確保と解放をそのまま `HeapAllocator` に委譲するだけのため `Allocator` の契約を満たす。
    unsafe impl Allocator for FailAfter {
        fn alloc(&mut self, layout: std::alloc::Layout) -> Option<std::ptr::NonNull<u8>> {
            if self.remaining == 0 {
                return None;
            }
            self.remaining -= 1;
            HeapAllocator.alloc(layout)
        }
        unsafe fn dealloc(&mut self, ptr: std::ptr::NonNull<u8>, layout: std::alloc::Layout) {
            unsafe { HeapAllocator.dealloc(ptr, layout) }
        }
    }
    let deep = nested(4, 2000, Value::None);
    for remaining in 0..4 {
        let mut alloc = FailAfter { remaining };
        let mut m = Builder::new(Limits::default(), &mut alloc);
        assert_eq!(
            m.bind("x", &deep).unwrap_err().kind,
            ErrorKind::TooMuchMemory
        );
        assert_eq!(m.live_bytes(), 0);
    }
    // すでに取り込んだ別引数と、変換中の兄弟の両方を回収する。
    let mut alloc = HeapAllocator;
    let mut m = Builder::new(Limits::default(), &mut alloc);
    m.eval("(defn accept (a b) 1)").unwrap();
    let mut m = m.freeze().unwrap();
    let before = m.live_bytes();
    let bad = Value::array(vec![deep.clone(), Value::Builtin("invalid")]);
    assert_eq!(
        m.call("accept", &[deep, bad]).unwrap_err().kind,
        ErrorKind::Runtime
    );
    assert_eq!(m.live_bytes(), before);
    assert!(m.call("accept", &[Value::None, Value::None]).unwrap() == Value::Int(1));
}

#[test]
fn deep_values_on_small_stacks() {
    if let Some(bytes) = std::env::var_os("RIN_DEEP_VALUE_STACK") {
        std::thread::Builder::new()
            .stack_size(bytes.to_str().unwrap().parse().unwrap())
            .spawn(|| {
                deep_roundtrips();
                schemas_and_ffi();
                failure_cleanup();
                allocator_failures();
            })
            .unwrap()
            .join()
            .unwrap();
        return;
    }
    for bytes in [256 * 1024, 1024 * 1024] {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "deep_values_on_small_stacks", "--nocapture"])
            .env("RIN_DEEP_VALUE_STACK", bytes.to_string())
            .status()
            .unwrap();
        assert!(
            status.success(),
            "deep value child failed with stack {bytes}: {status}"
        );
    }
}

#[test]
fn formatting_and_schema_error_order() {
    let flat = Value::list((0..20).map(Value::Int).collect());
    let pretty = format!("{flat:#?}");
    // Debug は Cons の tail も深さに数え、境界以降を省略せず一行で書く。
    assert!(pretty.contains("tail: Cons { head: Int(15)"));
    let compact_whitespace =
        |s: String| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    // pretty の末尾のカンマは Debug builder が付けるため取り除いて比べる。
    assert_eq!(
        compact_whitespace(pretty)
            .replace(",)", ")")
            .replace(",}", "}"),
        compact_whitespace(format!("{flat:?}"))
    );
    assert_eq!(
        format!("{:#?}", Value::some(Value::Int(7))),
        "Some(\n    Int(\n        7,\n    ),\n)"
    );
    let cyclic = Schema::parse("(defschema A B) (defschema B A) A").unwrap();
    assert!(
        cyclic
            .check(&Value::None)
            .unwrap_err()
            .message
            .contains("cyclic schema reference")
    );
    let v = Value::structure(
        "S",
        BTreeMap::from([
            ("a".into(), Value::list(vec![Value::Int(1), Value::None])),
            ("b".into(), Value::some(Value::Bool(true))),
        ]),
    );
    assert_eq!(v.to_string(), "S { a: [1 none] b: some(true) }");
    assert_eq!(
        format!("{:?}", Value::list(vec![Value::Int(1)])),
        "List(Cons { head: Int(1), tail: Nil, len: 1 })"
    );
    let err = Schema::parse("(struct S (a (list bool)) (missing i64))")
        .unwrap()
        .check(&v)
        .unwrap_err();
    assert_eq!(err.message, "$.a[0]: expected bool, got i64");
    let err = Schema::parse("(struct S (a any))")
        .unwrap()
        .check(&v)
        .unwrap_err();
    assert_eq!(err.message, "$: unexpected field 'b' on struct S");
}

#[test]
fn shallow_and_iterative_paths_agree() {
    let mut alloc = HeapAllocator;
    let mut m = Builder::new(Limits::default(), &mut alloc);
    for depth in [7, 8, 9, 15, 16, 17, 31, 32, 33, 64] {
        for shape in 0..8 {
            let value = nested(shape, depth, Value::Int(7));
            let other = nested(shape, depth, Value::Int(7));
            assert!(value == other);
            assert!(value != nested(shape, depth, Value::Int(8)));
            m.bind("x", &value).unwrap();
            assert!(m.eval("x").unwrap() == value);
            assert_eq!(value.to_string(), other.to_string());
            assert_eq!(format!("{value:?}"), format!("{other:?}"));
            let nan = nested(shape, depth, Value::Float(f64::NAN));
            assert!(nan != nan.clone(), "shared NaN at depth {depth}");

            let before = m.live_bytes();
            let bad = nested(
                shape,
                depth,
                Value::array(vec![
                    Value::str("allocated sibling"),
                    Value::Builtin("invalid"),
                ]),
            );
            assert_eq!(m.bind("bad", &bad).unwrap_err().kind, ErrorKind::Runtime);
            assert_eq!(m.live_bytes(), before, "failed import at depth {depth}");
            assert!(m.eval("42").unwrap() == Value::Int(42));
        }
    }
}
