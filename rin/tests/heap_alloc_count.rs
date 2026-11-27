//! システムヒープからの確保回数を計測・禁止する検証テスト。
//!
//! #19 の要件に従い、Rust ホスト境界 API (`Builder` / `Machine` / `ValueRef` / `ErrorRef`)、
//! C FFI、エラー・バックトレース経路、`ArenaAllocator` の定常使用量を検証する。

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ffi::CStr;
use std::fmt::{self, Write};

use rin::ffi::{
    RinAllocator, RinBinding, RinErrorKind, RinTag, rin_as_bool, rin_as_float, rin_as_int,
    rin_as_text, rin_bind, rin_bool, rin_build, rin_call, rin_call_in, rin_car, rin_cdr, rin_cons,
    rin_copy_f64s, rin_copy_i64s, rin_drop, rin_eval, rin_eval_with, rin_f64s, rin_field,
    rin_float, rin_free, rin_freeze, rin_i64s, rin_int, rin_last_error, rin_len,
    rin_limits_default, rin_load, rin_memory_config_default, rin_new_with_config, rin_nil,
    rin_none, rin_some, rin_struct, rin_struct_set, rin_tag, rin_text, rin_tuple_get, rin_unwrap,
    rin_variant, rin_variant_name,
};
use rin::{
    Allocator, ArenaAllocator, Builder, ErrorKind, Limits, MemoryConfig, RegionConfig, ValueTag,
};

struct CountingAllocator;

thread_local! {
    static COUNTING_ENABLED: Cell<bool> = const { Cell::new(false) };
    static FORBID_HEAP: Cell<bool> = const { Cell::new(false) };
    static THREAD_ALLOCS: Cell<usize> = const { Cell::new(0) };
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

impl CountingAllocator {
    #[inline]
    fn on_alloc() -> bool {
        if COUNTING_ENABLED.with(|c| c.get()) {
            THREAD_ALLOCS.with(|n| n.set(n.get() + 1));
        }
        !FORBID_HEAP.with(|f| f.get())
    }
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !Self::on_alloc() {
            return std::ptr::null_mut();
        }
        // SAFETY: システムアロケータへそのまま委譲する。
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if !Self::on_alloc() {
            return std::ptr::null_mut();
        }
        // SAFETY: システムアロケータへそのまま委譲する。
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: システムアロケータへそのまま委譲する。
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if !Self::on_alloc() {
            return std::ptr::null_mut();
        }
        // SAFETY: システムアロケータへそのまま委譲する。
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// 渡されたクロージャ実行中のシステムヒープ確保回数を返す。
fn measure_heap_allocs<R>(f: impl FnOnce() -> R) -> (R, usize) {
    THREAD_ALLOCS.with(|n| n.set(0));
    COUNTING_ENABLED.with(|c| c.set(true));
    let res = f();
    COUNTING_ENABLED.with(|c| c.set(false));
    let count = THREAD_ALLOCS.with(|n| n.get());
    (res, count)
}

/// システムヒープ確保を完全に禁止（NULL 返却）かつカウント 0 を強制してクロージャを実行する。
fn forbid_heap<R>(f: impl FnOnce() -> R) -> R {
    THREAD_ALLOCS.with(|n| n.set(0));
    COUNTING_ENABLED.with(|c| c.set(true));
    FORBID_HEAP.with(|b| b.set(true));
    let res = f();
    FORBID_HEAP.with(|b| b.set(false));
    COUNTING_ENABLED.with(|c| c.set(false));
    assert_eq!(THREAD_ALLOCS.with(|n| n.get()), 0);
    res
}

struct FixedWriter<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> FixedWriter<N> {
    const fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
        }
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.buf[..self.len]).unwrap()
    }
}

impl<const N: usize> Write for FixedWriter<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        let copy = bytes.len().min(N - self.len);
        self.buf[self.len..self.len + copy].copy_from_slice(&bytes[..copy]);
        self.len += copy;
        Ok(())
    }
}

#[test]
fn counting_allocator_detects_std_heap_allocations() {
    let (_, count) = measure_heap_allocs(|| {
        let v: Vec<u8> = vec![1, 2, 3, 4];
        std::hint::black_box(v);
    });
    assert!(count >= 1);
}

#[test]
fn arena_allocator_itself_uses_zero_system_heap() {
    let mut backing = vec![0u8; 4096];
    let (used, count) = measure_heap_allocs(|| {
        let mut arena = ArenaAllocator::new(&mut backing);
        let p = rin::Allocator::alloc(&mut arena, Layout::from_size_align(128, 8).unwrap());
        assert!(p.is_some());
        arena.used()
    });
    assert_eq!(count, 0);
    assert_eq!(used, 128);
}

#[test]
fn zero_heap_full_rust_lifecycle_and_error_paths() {
    let mut backing = vec![0u8; 512 * 1024];
    let mut out = FixedWriter::<4096>::new();

    forbid_heap(|| {
        let mut arena = ArenaAllocator::new(&mut backing);
        let mut builder = Builder::new(Limits::default(), &mut arena);

        builder
            .load_module_ref(
                "cards",
                r#"
                (export make_card total_cost boom)
                (defn helper (x) (* x 2))
                (defn make_card (name cost)
                  Card { name: name cost: (helper cost) kind: Kind.Spell(cost) })
                (defn total_cost (xs)
                  (fold (fn (acc c) (+ acc c.cost)) 0
                    (sort_by (fn (c) c.cost)
                      (filter (fn (c) (> c.cost 0)) xs))))
                (defn inner (n) (if (== n 0) (/ 1 0) (+ 1 (inner (- n 1)))))
                (defn boom (n) (inner n))
                "#,
            )
            .unwrap();

        let s = builder.val_text("fireball").unwrap();
        let i3 = builder.val_int(3);
        let opt = builder.val_some(i3).unwrap();
        let ok_v = builder.val_ok(opt).unwrap();
        let err_v = builder.val_err(s).unwrap();
        let tup = builder.val_tuple(&[s, i3]).unwrap();
        let arr = builder
            .val_array(&[builder.val_int(1), builder.val_int(2)])
            .unwrap();
        let lst = builder.val_list(&[i3, builder.val_int(5)]).unwrap();
        let st = builder
            .val_struct("Meta", &[("tag", s), ("count", i3)])
            .unwrap();
        let var = builder.val_variant("Flag", "On", Some(i3)).unwrap();
        builder.bind_val("meta", st).unwrap();
        builder.bind_val("nums", lst).unwrap();

        assert_eq!(builder.value_ref(opt).tag(), ValueTag::Some);
        assert_eq!(builder.value_ref(ok_v).tag(), ValueTag::Ok);
        assert_eq!(builder.value_ref(err_v).tag(), ValueTag::Err);
        assert_eq!(builder.value_ref(tup).tag(), ValueTag::Tuple);
        assert_eq!(builder.value_ref(arr).tag(), ValueTag::Array);
        assert_eq!(builder.value_ref(var).tag(), ValueTag::Variant);

        builder.release_val(s);
        builder.release_val(opt);
        builder.release_val(ok_v);
        builder.release_val(err_v);
        builder.release_val(tup);
        builder.release_val(arr);
        builder.release_val(lst);
        builder.release_val(st);
        builder.release_val(var);

        builder
            .eval_ref("(defn add_meta (n) (+ n meta.count))")
            .unwrap();

        let mut machine = builder.into_machine();

        // eval_ref + Display into FixedWriter
        {
            let v = machine.eval_ref("(cards.make_card \"bolt\" 2)").unwrap();
            assert_eq!(v.tag(), ValueTag::Struct);
            let sr = v.as_struct().unwrap();
            assert_eq!(sr.name(), "Card");
            assert_eq!(sr.field("cost").unwrap().as_int(), Some(4));
            out.clear();
            write!(out, "{v}").unwrap();
            assert!(out.as_str().contains("Card"));
        }

        // call_ref + call_in_ref + eval_with_vals
        {
            let a = machine.val_int(10);
            let v = machine.call_ref("add_meta", &[a]).unwrap();
            assert_eq!(v.as_int(), Some(13));
            machine.release_val(a);

            let cname = machine.val_text("ice").unwrap();
            let ccost = machine.val_int(4);
            let c1 = machine
                .call_in_ref("cards", "make_card", &[cname, ccost])
                .unwrap();
            assert_eq!(
                c1.as_struct().unwrap().field("cost").unwrap().as_int(),
                Some(8)
            );
            machine.release_val(cname);

            let bv = machine.val_int(99);
            let v2 = machine
                .eval_with_vals("(+ tmp meta.count)", &[("tmp", bv)])
                .unwrap();
            assert_eq!(v2.as_int(), Some(102));
            machine.release_val(bv);

            // quote / quasiquote / ident のゼロヒープ評価
            let qv = machine.eval_ref("'(a b [1 2] Point { x: 3 })").unwrap();
            assert_eq!(qv.tag(), ValueTag::List);
            assert_eq!(qv.list_len(), Some(4));
            let qqv = machine
                .eval_ref("(let (xs [2 3]) `(1 ,@xs ,(+ 2 2)))")
                .unwrap();
            assert_eq!(qqv.tag(), ValueTag::List);
            assert_eq!(qqv.list_len(), Some(4));
            let idv = machine.eval_ref("'hello").unwrap();
            assert_eq!(idv.tag(), ValueTag::Ident);
            assert_eq!(idv.as_ident(), Some("hello"));

            // テンプレート埋め込みと浮動小数点表示のゼロヒープ評価
            let tmpl = machine
                .eval_ref("(let (n 42 x 1.5e-8 id 'foo) \"n=${n} x=${x} id=${id} z=${-0.0}\")")
                .unwrap();
            assert_eq!(tmpl.tag(), ValueTag::Text);
            assert_eq!(tmpl.as_text(), Some("n=42 x=1.5e-8 id=foo z=-0.0"));
            out.clear();
            write!(out, "{tmpl}").unwrap();
            assert_eq!(out.as_str(), "\"n=42 x=1.5e-8 id=foo z=-0.0\"");
            let fv = machine.eval_ref("1.0").unwrap();
            out.clear();
            write!(out, "{fv}").unwrap();
            assert_eq!(out.as_str(), "1.0");

            // 複数行文字列リテラル・埋め込みのゼロヒープ評価
            let ml = machine
                .eval_ref("(let (x 7) \"\"\"\n  a=${x}\n    b=${(+ x\n1)}\n  \"\"\")")
                .unwrap();
            assert_eq!(ml.tag(), ValueTag::Text);
            assert_eq!(ml.as_text(), Some("a=7\n  b=8"));
        }

        // Error paths + backtrace + suggestions + Display into FixedWriter
        {
            let e = machine.eval_ref("\"").unwrap_err();
            assert_eq!(e.kind(), ErrorKind::Lex);
            out.clear();
            write!(out, "{e}").unwrap();
            assert!(out.as_str().contains("[src.string]"));
        }
        {
            let e = machine.eval_ref("(if 1").unwrap_err();
            assert_eq!(e.kind(), ErrorKind::Parse);
            out.clear();
            write!(out, "{e}").unwrap();
            assert!(out.as_str().contains("[src.syntax]"));
        }
        {
            let e = machine.eval_ref("(cards.boom 5)").unwrap_err();
            assert_eq!(e.kind(), ErrorKind::Runtime);
            assert_eq!(e.module(), Some("cards"));
            assert!(e.backtrace_len() >= 2);
            out.clear();
            write!(out, "{e}").unwrap();
            assert!(out.as_str().contains("division by zero"));
            assert!(out.as_str().contains("cards.inner"));
        }
        {
            let e = machine.eval_ref("add_met").unwrap_err();
            assert_eq!(e.kind(), ErrorKind::Runtime);
            assert!(e.message().contains("did you mean 'add_meta'?"));
        }
        {
            let e = machine.eval_ref("meta.missing").unwrap_err();
            assert_eq!(e.kind(), ErrorKind::Runtime);
            assert!(e.message().contains("fields: count, tag"));
        }

        drop(machine);

        // 資源上限エラー（TooDeep / TooManySteps / TooMuchMemory）もゼロヒープでバックトレース付き返却
        {
            let mut arena2 = ArenaAllocator::new(&mut backing);
            let mut b = Builder::with_memory_config(
                Limits {
                    steps: 500,
                    depth: 12,
                    memory_bytes: 256 * 1024,
                },
                MemoryConfig {
                    perm: RegionConfig::new(2048, 16 * 1024),
                    obj: RegionConfig::new(2048, 16 * 1024),
                    work: RegionConfig::new(2048, 16 * 1024),
                },
                &mut arena2,
            );
            b.eval_ref(
                r#"
                (defn deep (n) (if (== n 0) 0 (+ 1 (deep (- n 1)))))
                (defn spin (n) (if (== n 0) 0 (spin (- n 1))))
                "#,
            )
            .unwrap();
            let mut m = b.into_machine();

            let e_deep = m.eval_ref("(deep 50)").unwrap_err();
            assert_eq!(e_deep.kind(), ErrorKind::TooDeep);
            assert!(e_deep.backtrace_len() >= 2);
            out.clear();
            write!(out, "{e_deep}").unwrap();
            assert!(out.as_str().contains("deep"));

            let e_steps = m.eval_ref("(spin 10000)").unwrap_err();
            assert_eq!(e_steps.kind(), ErrorKind::TooManySteps);
            assert!(e_steps.backtrace_len() >= 1);
            out.clear();
            write!(out, "{e_steps}").unwrap();
            assert!(out.as_str().contains("step limit exceeded"));

            drop(m);
        }
        {
            let mut arena3 = ArenaAllocator::new(&mut backing);
            let mut b = Builder::with_memory_config(
                Limits {
                    steps: 100_000,
                    depth: 200,
                    memory_bytes: 24 * 1024,
                },
                MemoryConfig {
                    perm: RegionConfig::new(2048, 8 * 1024),
                    obj: RegionConfig::new(2048, 8 * 1024),
                    work: RegionConfig::new(2048, 8 * 1024),
                },
                &mut arena3,
            );
            b.eval_ref("(defn blowup (n) (range 0 n))").unwrap();
            let mut m = b.into_machine();

            let e_mem = m.eval_ref("(blowup 10000)").unwrap_err();
            assert_eq!(e_mem.kind(), ErrorKind::TooMuchMemory);
            assert!(e_mem.backtrace_len() >= 1);
            out.clear();
            write!(out, "{e_mem}").unwrap();
            assert!(out.as_str().contains("blowup"));

            drop(m);
        }
    });
}

#[test]
fn zero_heap_full_c_ffi_lifecycle() {
    struct BumpCtx {
        buf: *mut u8,
        cap: usize,
        offset: usize,
    }

    unsafe extern "C" fn c_alloc(ctx: *mut std::ffi::c_void, size: usize, align: usize) -> *mut u8 {
        if size == 0 {
            return std::ptr::NonNull::<u8>::dangling().as_ptr();
        }
        let bump = unsafe { &mut *(ctx as *mut BumpCtx) };
        let aligned = (bump.offset + align - 1) & !(align - 1);
        if aligned + size > bump.cap {
            return std::ptr::null_mut();
        }
        bump.offset = aligned + size;
        unsafe { bump.buf.add(aligned) }
    }

    let mut backing = vec![0u8; 256 * 1024];

    forbid_heap(|| {
        let mut ctx = BumpCtx {
            buf: backing.as_mut_ptr(),
            cap: backing.len(),
            offset: 0,
        };
        let alloc = RinAllocator {
            ctx: (&mut ctx as *mut BumpCtx).cast(),
            alloc: Some(c_alloc),
            dealloc: None,
        };
        let m = rin_new_with_config(rin_limits_default(), rin_memory_config_default(), alloc);
        assert!(!m.is_null());

        unsafe {
            let lr = rin_load(
                m,
                c"mod_a".as_ptr(),
                c"(defn f (x) (+ x 1)) (defn boom () (/ 1 0))".as_ptr(),
            );
            assert!(lr.ok);

            let v_int = rin_int(m, 41);
            let v_flt = rin_float(m, 1.5);
            let v_bl = rin_bool(m, true);
            let v_str = rin_text(m, c"hi".as_ptr());
            let v_i64s = rin_i64s(m, [1, 2, 3].as_ptr(), 3);
            let v_f64s = rin_f64s(m, [1.0, 2.0].as_ptr(), 2);
            let v_nil = rin_nil(m);
            let v_lst = rin_cons(m, v_int, v_nil);
            let v_none = rin_none(m);
            let v_some = rin_some(m, v_int);
            let v_st = rin_struct(m, c"S".as_ptr());
            assert!(rin_struct_set(m, v_st, c"a".as_ptr(), v_int));
            let v_var = rin_variant(m, c"E".as_ptr(), c"A".as_ptr(), v_int);

            assert_eq!(rin_tag(m, v_int), RinTag::Int);
            let mut n = 0;
            assert!(rin_as_int(m, v_int, &mut n) && n == 41);
            let mut x = 0.0;
            assert!(rin_as_float(m, v_flt, &mut x) && x == 1.5);
            let mut b = false;
            assert!(rin_as_bool(m, v_bl, &mut b) && b);
            let mut slen = 0;
            let sptr = rin_as_text(m, v_str, &mut slen);
            assert_eq!(std::slice::from_raw_parts(sptr, slen), b"hi");
            assert_eq!(rin_len(m, v_lst), 1);
            let h = rin_car(m, v_lst);
            let t = rin_cdr(m, v_lst);
            assert_eq!(rin_len(m, t), 0);
            rin_drop(m, h);
            rin_drop(m, t);
            let sf = rin_field(m, v_st, c"a".as_ptr());
            assert!(rin_as_int(m, sf, &mut n) && n == 41);
            rin_drop(m, sf);
            let mut vlen = 0;
            let vptr = rin_variant_name(m, v_var, &mut vlen);
            assert_eq!(std::slice::from_raw_parts(vptr, vlen), b"A");
            let un = rin_unwrap(m, v_some);
            assert!(rin_as_int(m, un, &mut n) && n == 41);
            rin_drop(m, un);
            let elem = rin_tuple_get(m, v_i64s, 1);
            assert!(rin_as_int(m, elem, &mut n) && n == 2);
            rin_drop(m, elem);
            let mut ibuf = [0i64; 3];
            assert_eq!(rin_copy_i64s(m, v_i64s, ibuf.as_mut_ptr(), 3), 3);
            let mut fbuf = [0.0f64; 2];
            assert_eq!(rin_copy_f64s(m, v_f64s, fbuf.as_mut_ptr(), 2), 2);

            assert!(rin_bind(m, c"g".as_ptr(), v_int));
            let br = rin_build(m, c"(defn add_g (k) (+ g k))".as_ptr());
            assert!(br.ok);
            assert!(rin_freeze(m));

            let er = rin_eval(m, c"(mod_a.f g)".as_ptr());
            assert!(er.ok && rin_as_int(m, er.value, &mut n) && n == 42);

            let binds = [RinBinding {
                name: c"g".as_ptr(),
                value: v_int,
            }];
            let wr = rin_eval_with(m, c"(+ g 1)".as_ptr(), binds.as_ptr(), 1);
            assert!(wr.ok && rin_as_int(m, wr.value, &mut n) && n == 42);

            let args = [v_int as *const _];
            let cr = rin_call(m, c"add_g".as_ptr(), args.as_ptr(), 1);
            assert!(cr.ok && rin_as_int(m, cr.value, &mut n) && n == 82);

            let cir = rin_call_in(m, c"mod_a".as_ptr(), c"f".as_ptr(), args.as_ptr(), 1);
            assert!(cir.ok && rin_as_int(m, cir.value, &mut n) && n == 42);

            // Runtime error with backtrace
            let err_r = rin_call_in(m, c"mod_a".as_ptr(), c"boom".as_ptr(), std::ptr::null(), 0);
            assert!(!err_r.ok);
            let err = rin_last_error(m);
            assert!(!err.is_null());
            assert_eq!((*err).kind, RinErrorKind::Runtime);
            assert!(!(*err).backtrace.is_null());
            assert!(
                CStr::from_ptr((*err).text)
                    .to_str()
                    .unwrap()
                    .contains("division by zero")
            );

            rin_drop(m, v_int);
            rin_drop(m, v_flt);
            rin_drop(m, v_bl);
            rin_drop(m, v_str);
            rin_drop(m, v_i64s);
            rin_drop(m, v_f64s);
            rin_drop(m, v_nil);
            rin_drop(m, v_lst);
            rin_drop(m, v_none);
            rin_drop(m, v_some);
            rin_drop(m, v_st);
            rin_drop(m, v_var);

            rin_free(m);
        }
        assert!(ctx.offset > 0);
    });
}

#[test]
fn val_list_does_not_leak_objects() {
    let mut backing = vec![0u8; 64 * 1024];
    forbid_heap(|| {
        let mut arena = ArenaAllocator::new(&mut backing);
        let mut builder = Builder::new(Limits::default(), &mut arena);

        // スカラ列
        let a = builder.val_int(1);
        let b = builder.val_int(2);
        let c = builder.val_int(3);
        let lst = builder.val_list(&[a, b, c]).unwrap();
        assert_eq!(builder.value_ref(lst).list_len(), Some(3));
        builder.release_val(lst);
        assert_eq!(builder.live_bytes(), 0);

        // 文字列と入れ子リスト（参照カウント共有を含む）
        let s1 = builder.val_text("alpha").unwrap();
        let s2 = builder.val_text("beta").unwrap();
        let inner = builder.val_list(&[s1, s2]).unwrap();
        let outer = builder.val_list(&[inner, s1, s2]).unwrap();
        assert_eq!(builder.value_ref(outer).list_len(), Some(3));
        builder.release_val(outer);
        builder.release_val(inner);
        builder.release_val(s1);
        builder.release_val(s2);
        assert_eq!(builder.live_bytes(), 0);

        // 途中 OOM 時にも構築済みセル・要素をリークしない
        drop(builder);
        let mut small_buf = [0u8; 32 * 1024];
        let mut small_arena = ArenaAllocator::new(&mut small_buf);
        let mut small = Builder::with_memory_config(
            Limits {
                memory_bytes: 10 * 1024,
                ..Limits::default()
            },
            MemoryConfig {
                perm: RegionConfig::new(512, 1024),
                obj: RegionConfig::new(512, 1024),
                work: RegionConfig::new(512, 1024),
            },
            &mut small_arena,
        );
        let item = small.val_text("payload").unwrap();
        let base_live = small.live_bytes();
        let items = [item; 512];
        let err = small.val_list(&items).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::TooMuchMemory);
        assert_eq!(small.live_bytes(), base_live);
        small.release_val(item);
        assert_eq!(small.live_bytes(), 0);
    });
}

#[test]
fn arena_usage_plateaus_across_thousands_of_evaluations() {
    struct TrackedArena<'a> {
        inner: ArenaAllocator<'a>,
        used_cell: &'a Cell<usize>,
    }

    // SAFETY: 確保と解放を `ArenaAllocator` に委譲し使用量を記録するだけのため `Allocator` の契約を満たす。
    unsafe impl Allocator for TrackedArena<'_> {
        fn alloc(&mut self, layout: Layout) -> Option<std::ptr::NonNull<u8>> {
            let p = self.inner.alloc(layout);
            self.used_cell.set(self.inner.used());
            p
        }

        unsafe fn dealloc(&mut self, ptr: std::ptr::NonNull<u8>, layout: Layout) {
            unsafe { self.inner.dealloc(ptr, layout) };
        }
    }

    let mut backing = vec![0u8; 256 * 1024];
    let used_cell = Cell::new(0usize);
    let mut arena = TrackedArena {
        inner: ArenaAllocator::new(&mut backing),
        used_cell: &used_cell,
    };

    let mut builder = Builder::new(Limits::default(), &mut arena);
    builder
        .eval_ref(
            r#"
            (defn work (n)
              (let (xs (range 0 n)
                    ys (map (fn (x) Item { k: x label: "item" }) xs)
                    zs (filter (fn (it) (== (% it.k 2) 0)) ys))
                (fold (fn (acc it) (+ acc it.k)) 0 zs)))
            "#,
        )
        .unwrap();
    let mut machine = builder.into_machine();
    let baseline_live = machine.live_bytes();

    // ウォームアップ（WorkArena / ObjHeap / Pool チャンクを定常サイズまで確保）
    for i in 0..64 {
        let a = machine.val_int(8);
        let r = machine.call_ref("work", &[a]).unwrap();
        assert_eq!(r.as_int(), Some(12));
        machine.release_val(a);

        let s = machine.val_text("elem").unwrap();
        let lst = machine.val_list(&[s, machine.val_int(i as i64)]).unwrap();
        let v = machine.eval_with_vals("(len xs)", &[("xs", lst)]).unwrap();
        assert_eq!(v.as_int(), Some(2));
        machine.release_val(lst);
        machine.release_val(s);

        let e = machine
            .eval_ref("(fold + 0 (map (fn (x) (* x 2)) [1 2 3 4 5]))")
            .unwrap();
        assert_eq!(e.as_int(), Some(30));

        if i % 4 == 0 {
            assert!(machine.eval_ref("(/ 1 0)").is_err());
        }
    }
    let after_warmup_live = machine.live_bytes();
    let after_warmup_arena = used_cell.get();
    assert!(after_warmup_live <= baseline_live + 128);

    // 30,000 回反復しても ArenaAllocator の新規確保が 0 バイト（完全定常）であること。
    for i in 0..30_000 {
        let a = machine.val_int(8);
        let r = machine.call_ref("work", &[a]).unwrap();
        assert_eq!(r.as_int(), Some(12));
        machine.release_val(a);

        let s = machine.val_text("elem").unwrap();
        let lst = machine.val_list(&[s, machine.val_int(i as i64)]).unwrap();
        let v = machine.eval_with_vals("(len xs)", &[("xs", lst)]).unwrap();
        assert_eq!(v.as_int(), Some(2));
        machine.release_val(lst);
        machine.release_val(s);

        let e = machine
            .eval_ref("(fold + 0 (map (fn (x) (* x 2)) [1 2 3 4 5]))")
            .unwrap();
        assert_eq!(e.as_int(), Some(30));

        if i % 100 == 0 {
            assert!(machine.eval_ref("(/ 1 0)").is_err());
        }
    }
    assert!(machine.live_bytes() <= after_warmup_live);
    assert_eq!(
        used_cell.get(),
        after_warmup_arena,
        "arena grew after warmup: warmup={after_warmup_arena}, after={}",
        used_cell.get()
    );
    drop(machine);
}
