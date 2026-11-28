//! steps=0でも行う解析・コンパイルのピークメモリを、複数の入力サイズで検証する。
#[path = "../examples/compile_memory/mod.rs"]
mod memory;
use rin::{Builder, ErrorKind, HeapAllocator, Limits};

#[test]
fn preparation_memory_scales_with_source_size() {
    for n in [100, 1000, 10_000] {
        let source = (0..n)
            .map(|i| format!("(defn f{i:08} ((x i64)) -> i64 (+ x {i}))\n"))
            .collect::<String>();
        let mut alloc = HeapAllocator;
        let mut machine = Builder::new(
            Limits {
                steps: 0,
                memory_bytes: 64 * 1024 * 1024,
                ..Limits::default()
            },
            &mut alloc,
        );
        let baseline = memory::start();
        let error = machine.eval(&source).unwrap_err();
        let peak = memory::peak(baseline);
        assert_eq!(error.kind, ErrorKind::TooManySteps);
        assert_eq!(machine.live_bytes(), 0);
        // 時間はCI負荷に依存するため閾値にしない。記録の二乗増加を検出する。
        assert!(peak <= 128 * source.len() + 65_536, "n={n}, peak={peak}");
    }
}
