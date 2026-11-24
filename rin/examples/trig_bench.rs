//! `sin`・`cos` の決定的な実装（Strict / Fused）と標準ライブラリの速度比較、
//! および精度検証用のビット列出力を行う。

use std::hint::black_box;
use std::time::Instant;

use rin::math::{self, MathFma};

fn bench_median_ns(mut f: impl FnMut() -> f64, iters: usize, samples: usize) -> f64 {
    let mut times = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start = Instant::now();
        let mut acc = 0.0;
        for _ in 0..iters {
            acc += f();
        }
        black_box(acc);
        times.push(start.elapsed().as_nanos() as f64 / iters as f64);
    }
    times.sort_by(|a, b| a.total_cmp(b));
    times[samples / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--eval-hex") {
        // 標準入力から 16 進ビット列を読み、func x_hex strict_hex fused_hex を出力する。
        use std::io::{self, BufRead, Write};
        let stdin = io::stdin();
        let mut stdout = io::BufWriter::new(io::stdout().lock());
        for line in stdin.lock().lines() {
            let line = line.unwrap();
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let bits = u64::from_str_radix(trimmed.trim_start_matches("0x"), 16).unwrap();
            let x = f64::from_bits(bits);
            let ss = math::sin(x, MathFma::Strict).to_bits();
            let sf = math::sin(x, MathFma::Fused).to_bits();
            let cs = math::cos(x, MathFma::Strict).to_bits();
            let cf = math::cos(x, MathFma::Fused).to_bits();
            writeln!(stdout, "sin\t0x{bits:016x}\t0x{ss:016x}\t0x{sf:016x}").unwrap();
            writeln!(stdout, "cos\t0x{bits:016x}\t0x{cs:016x}\t0x{cf:016x}").unwrap();
        }
        return;
    }

    let cases: [(&str, Vec<f64>); 3] = [
        (
            "small (|x| <= pi/4)",
            (0..256).map(|i| (i as f64 - 128.0) * 0.005).collect(),
        ),
        (
            "medium (Cody-Waite)",
            (0..256).map(|i| (i as f64 + 1.0) * 1.23456789).collect(),
        ),
        (
            "large (Payne-Hanek)",
            (0..256)
                .map(|i| 1.0e12 + (i as f64) * 1.23456789e9)
                .collect(),
        ),
    ];

    let iters = 200_000;
    let samples = 9;
    println!("| 範囲 | 関数 | std (ns) | Strict (ns) | Fused (ns) | Strict/std | Fused/std |");
    println!("|---|---|---:|---:|---:|---:|---:|");
    for (label, xs) in &cases {
        let mut idx = 0usize;
        let std_sin = bench_median_ns(
            || {
                let x = black_box(xs[idx & 255]);
                idx += 1;
                x.sin()
            },
            iters,
            samples,
        );
        let strict_sin = bench_median_ns(
            || {
                let x = black_box(xs[idx & 255]);
                idx += 1;
                math::sin(x, MathFma::Strict)
            },
            iters,
            samples,
        );
        let fused_sin = bench_median_ns(
            || {
                let x = black_box(xs[idx & 255]);
                idx += 1;
                math::sin(x, MathFma::Fused)
            },
            iters,
            samples,
        );
        println!(
            "| {label} | sin | {std_sin:.2} | {strict_sin:.2} | {fused_sin:.2} | {:.2}x | {:.2}x |",
            strict_sin / std_sin,
            fused_sin / std_sin
        );

        let std_cos = bench_median_ns(
            || {
                let x = black_box(xs[idx & 255]);
                idx += 1;
                x.cos()
            },
            iters,
            samples,
        );
        let strict_cos = bench_median_ns(
            || {
                let x = black_box(xs[idx & 255]);
                idx += 1;
                math::cos(x, MathFma::Strict)
            },
            iters,
            samples,
        );
        let fused_cos = bench_median_ns(
            || {
                let x = black_box(xs[idx & 255]);
                idx += 1;
                math::cos(x, MathFma::Fused)
            },
            iters,
            samples,
        );
        println!(
            "| {label} | cos | {std_cos:.2} | {strict_cos:.2} | {fused_cos:.2} | {:.2}x | {:.2}x |",
            strict_cos / std_cos,
            fused_cos / std_cos
        );
    }
}
