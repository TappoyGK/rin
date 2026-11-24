//! プラットフォームとビルド設定によらずビット一致する数学関数を実装する。
//!
//! 計算手順・定数・FMA 適用位置の正本は `docs/math-spec.md` を参照すること。

/// 組み込み数学関数における積和融合（FMA）の使用方針。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MathFma {
    /// FMA を使わず、IEEE 754 の四則演算のみで計算する（既定）。
    #[default]
    Strict,
    /// `docs/math-spec.md` で定めた位置のみ IEEE 754 `fusedMultiplyAdd`（`f64::mul_add`）を使う。
    Fused,
}

// ---------------------------------------------------------------------------
// カーネル多項式係数（fdlibm / musl 互換の IEEE 754 binary64 定数）
// ---------------------------------------------------------------------------

const S1: f64 = f64::from_bits(0xbfc5_5555_5555_5549); // -1.66666666666666324348e-01
const S2: f64 = f64::from_bits(0x3f81_1111_1110_f8a6); //  8.33333333332248946124e-03
const S3: f64 = f64::from_bits(0xbf2a_01a0_19c1_61d5); // -1.98412698298579493134e-04
const S4: f64 = f64::from_bits(0x3ec7_1de3_57b1_fe7d); //  2.75573137070700676789e-06
const S5: f64 = f64::from_bits(0xbe5a_e5e6_8a2b_9ceb); // -2.50507602534068634195e-08
const S6: f64 = f64::from_bits(0x3de5_d93a_5acf_d57c); //  1.58969099521155010221e-10

const C1: f64 = f64::from_bits(0x3fa5_5555_5555_554c); //  4.16666666666666019037e-02
const C2: f64 = f64::from_bits(0xbf56_c16c_16c1_5177); // -1.38888888888741095749e-03
const C3: f64 = f64::from_bits(0x3efa_01a0_19cb_1590); //  2.48015872894767294178e-05
const C4: f64 = f64::from_bits(0xbe92_7e4f_809c_52ad); // -2.75573143513906633035e-07
const C5: f64 = f64::from_bits(0x3e21_ee9e_bdb4_b1c4); //  2.08757232129817482790e-09
const C6: f64 = f64::from_bits(0xbda8_fae9_be88_38d4); // -1.13596475577881948265e-11

// ---------------------------------------------------------------------------
// 引数縮約用定数（Cody-Waite 3 段縮約）
// ---------------------------------------------------------------------------

const INV_PIO2: f64 = f64::from_bits(0x3fe4_5f30_6dc9_c883); // 2/pi
const PIO2_1: f64 = f64::from_bits(0x3ff9_21fb_5440_0000); // 上位 33 bit
const PIO2_1T: f64 = f64::from_bits(0x3dd0_b461_1a62_6331); // pi/2 - PIO2_1
const PIO2_2: f64 = f64::from_bits(0x3dd0_b461_1a60_0000); // 中位 33 bit
const PIO2_2T: f64 = f64::from_bits(0x3ba3_198a_2e03_7073); // pi/2 - (PIO2_1 + PIO2_2)
const PIO2_3: f64 = f64::from_bits(0x3ba3_198a_2e00_0000); // 下位 33 bit
const PIO2_3T: f64 = f64::from_bits(0x397b_839a_2520_49c1); // pi/2 - (PIO2_1 + PIO2_2 + PIO2_3)

// ---------------------------------------------------------------------------
// Payne-Hanek 縮約用定数（先頭に 0 を 2 ワード置いた 2/pi の 32-bit ワード列）
// ---------------------------------------------------------------------------

const TWO_OVER_PI: [u32; 44] = [
    0x0000_0000,
    0x0000_0000,
    0xa2f9_836e,
    0x4e44_1529,
    0xfc27_57d1,
    0xf534_ddc0,
    0xdb62_9599,
    0x3c43_9041,
    0xfe51_63ab,
    0xdebb_c561,
    0xb724_6e3a,
    0x424d_d2e0,
    0x0649_2eea,
    0x09d1_921c,
    0xfe1d_eb1c,
    0xb129_a73e,
    0xe882_35f5,
    0x2ebb_4484,
    0xe99c_7026,
    0xb45f_7e41,
    0x3991_d639,
    0x8353_39f4,
    0x9c84_5f8b,
    0xbdf9_283b,
    0x1ff8_97ff,
    0xde05_980f,
    0xef2f_118b,
    0x5a0a_6d1f,
    0x6d36_7ecf,
    0x27cb_09b7,
    0x4f46_3f66,
    0x9e5f_ea2d,
    0x7527_bac7,
    0xebe5_f17b,
    0x3d07_39f7,
    0x8a52_92ea,
    0x6bfb_5fb1,
    0x1f8d_5d08,
    0x5603_3046,
    0xfc7b_6bab,
    0xf0cf_bc20,
    0x9af4_361d,
    0xa9e3_9161,
    0x5ee6_1b08,
];

// pi/2 を 2^127 倍して最近接丸めした 128-bit 整数（1 <= pi/2 < 2）
const PIO2_Q127_HI: u64 = 0xc90f_daa2_2168_c234;
const PIO2_Q127_LO: u64 = 0xc4c6_628b_80dc_1cd1;

/// 決定的な `sin(x)`。
pub fn sin(x: f64, mode: MathFma) -> f64 {
    let ix = ((x.to_bits() >> 32) as u32) & 0x7fff_ffff;

    // |x| <= pi/4
    if ix <= 0x3fe9_21fb {
        // |x| < 2^-26: sin(x) = x（-0.0 と非正規化数をそのまま保つ）
        if ix < 0x3e50_0000 {
            return x;
        }
        return kernel_sin(x, 0.0, false, mode);
    }

    // NaN または ±inf
    if ix >= 0x7ff0_0000 {
        return f64::NAN;
    }

    let (n, y0, y1) = rem_pio2(x);
    match n & 3 {
        0 => kernel_sin(y0, y1, true, mode),
        1 => kernel_cos(y0, y1, mode),
        2 => -kernel_sin(y0, y1, true, mode),
        _ => -kernel_cos(y0, y1, mode),
    }
}

/// 決定的な `cos(x)`。
pub fn cos(x: f64, mode: MathFma) -> f64 {
    let ix = ((x.to_bits() >> 32) as u32) & 0x7fff_ffff;

    // |x| <= pi/4
    if ix <= 0x3fe9_21fb {
        // |x| < 2^-27: cos(x) = 1.0
        if ix < 0x3e40_0000 {
            return 1.0;
        }
        return kernel_cos(x, 0.0, mode);
    }

    // NaN または ±inf
    if ix >= 0x7ff0_0000 {
        return f64::NAN;
    }

    let (n, y0, y1) = rem_pio2(x);
    match n & 3 {
        0 => kernel_cos(y0, y1, mode),
        1 => -kernel_sin(y0, y1, true, mode),
        2 => -kernel_cos(y0, y1, mode),
        _ => kernel_sin(y0, y1, true, mode),
    }
}

#[inline]
fn kernel_sin(x: f64, y: f64, iy: bool, mode: MathFma) -> f64 {
    let z = x * x;
    let w = z * z;
    let v = z * x;
    match mode {
        MathFma::Strict => {
            let r = S2 + z * (S3 + z * S4) + z * w * (S5 + z * S6);
            if !iy {
                x + v * (S1 + z * r)
            } else {
                x - ((z * (0.5 * y - v * r) - y) - v * S1)
            }
        }
        MathFma::Fused => {
            let r1 = z.mul_add(S4, S3);
            let r2 = z.mul_add(r1, S2);
            let r3 = z.mul_add(S6, S5);
            let r = (z * w).mul_add(r3, r2);
            if !iy {
                v.mul_add(z.mul_add(r, S1), x)
            } else {
                let t = v.mul_add(-r, 0.5 * y);
                let u = z.mul_add(t, -y);
                x - v.mul_add(-S1, u)
            }
        }
    }
}

#[inline]
fn kernel_cos(x: f64, y: f64, mode: MathFma) -> f64 {
    let z = x * x;
    let w = z * z;
    let hz = 0.5 * z;
    let w2 = 1.0 - hz;
    match mode {
        MathFma::Strict => {
            let r = z * (C1 + z * (C2 + z * C3)) + w * w * (C4 + z * (C5 + z * C6));
            w2 + (((1.0 - w2) - hz) + (z * r - x * y))
        }
        MathFma::Fused => {
            let p1 = z.mul_add(z.mul_add(C3, C2), C1);
            let p2 = z.mul_add(z.mul_add(C6, C5), C4);
            let r = (w * w).mul_add(p2, z * p1);
            let corr = z.mul_add(r, -(x * y));
            w2 + (((1.0 - w2) - hz) + corr)
        }
    }
}

/// 有限の `x`（`|x| > pi/4`）を `x = n * (pi/2) + y0 + y1`（`|y0 + y1| <= pi/4`）に縮約する。
fn rem_pio2(x: f64) -> (i32, f64, f64) {
    let bits = x.to_bits();
    let ix = ((bits >> 32) as u32) & 0x7fff_ffff;
    let neg = (bits >> 63) != 0;

    // 経路 1: pi/4 < |x| < 3*pi/4（n = ±1）
    if ix < 0x4002_d97c {
        if !neg {
            let z = x - PIO2_1;
            if ix != 0x3ff9_21fb {
                let y0 = z - PIO2_1T;
                let y1 = (z - y0) - PIO2_1T;
                return (1, y0, y1);
            } else {
                let z = z - PIO2_2;
                let y0 = z - PIO2_2T;
                let y1 = (z - y0) - PIO2_2T;
                return (1, y0, y1);
            }
        } else {
            let z = x + PIO2_1;
            if ix != 0x3ff9_21fb {
                let y0 = z + PIO2_1T;
                let y1 = (z - y0) + PIO2_1T;
                return (-1, y0, y1);
            } else {
                let z = z + PIO2_2;
                let y0 = z + PIO2_2T;
                let y1 = (z - y0) + PIO2_2T;
                return (-1, y0, y1);
            }
        }
    }

    // 経路 2: 3*pi/4 <= |x| <= 2^19 * pi/2（Cody-Waite 3 段縮約）
    if ix <= 0x4139_21fb {
        let t = x.abs();
        let n = (t * INV_PIO2 + 0.5) as i32;
        let fn_val = f64::from(n);
        let mut r = t - fn_val * PIO2_1;
        let mut w = fn_val * PIO2_1T;
        let mut y0 = r - w;
        let mut i = (ix as i32) - (((y0.to_bits() >> 32) as i32) & 0x7ff0_0000);
        if i > (16 << 20) {
            let t2 = r;
            w = fn_val * PIO2_2;
            r = t2 - w;
            w = fn_val * PIO2_2T - ((t2 - r) - w);
            y0 = r - w;
            i = (ix as i32) - (((y0.to_bits() >> 32) as i32) & 0x7ff0_0000);
            if i > (49 << 20) {
                let t3 = r;
                w = fn_val * PIO2_3;
                r = t3 - w;
                w = fn_val * PIO2_3T - ((t3 - r) - w);
                y0 = r - w;
            }
        }
        let y1 = (r - y0) - w;
        if neg {
            return (-n, -y0, -y1);
        }
        return (n, y0, y1);
    }

    // 経路 3: |x| > 2^19 * pi/2（整数 Payne-Hanek 縮約）
    rem_pio2_large(x)
}

/// 任意の大きい有限 `x`（`|x| > 2^19 * pi/2`）を固定長整数演算で縮約する。
fn rem_pio2_large(x: f64) -> (i32, f64, f64) {
    let bits = x.to_bits();
    let neg = (bits >> 63) != 0;
    let e = (((bits >> 52) & 0x7ff) as i32) - 1023; // 19 <= e <= 1023
    let m = (bits & 0x000f_ffff_ffff_ffff) | 0x0010_0000_0000_0000; // 53 bit

    let idx = (e / 32) as usize;
    let rem = (e % 32) as u32;

    // 2/pi * 2^e の該当区間から 7 ワード（224 bit）を切り出す。
    let mut w = [0u32; 7];
    for (j, slot) in w.iter_mut().enumerate() {
        let hi = TWO_OVER_PI[idx + j];
        *slot = if rem == 0 {
            hi
        } else {
            let lo = TWO_OVER_PI[idx + j + 1];
            (hi << rem) | (lo >> (32 - rem))
        };
    }

    // 224-bit 整数 W と 53-bit 整数 m の積 P = m * W（9 ワード = 288 bit、リトルエンディアン）
    let mut prod = [0u32; 9];
    let mut carry = 0u128;
    for j in 0..7 {
        let p = u128::from(m) * u128::from(w[6 - j]) + carry;
        prod[j] = p as u32;
        carry = p >> 32;
    }
    prod[7] = carry as u32;
    prod[8] = (carry >> 32) as u32;

    // 小数点は bit 212 の直下（prod[6] の bit 20）。
    // 象限 q = (P >> 212) & 3
    let mut q = ((prod[6] >> 20) & 3) as i32;

    // 小数部 212 bit（bit 211..0）を 256-bit 整数（hi: u128, lo: u128）の上位に左詰め（44 bit シフト）。
    // bit 211 -> hi の bit 127。
    let p_hi_top = u128::from(prod[6] & 0x000f_ffff); // bit 211..192 (20 bit)
    let p5 = u128::from(prod[5]); // bit 191..160
    let p4 = u128::from(prod[4]); // bit 159..128
    let p3 = u128::from(prod[3]); // bit 127..96
    let p2 = u128::from(prod[2]); // bit 95..64
    let p1 = u128::from(prod[1]); // bit 63..32
    let p0 = u128::from(prod[0]); // bit 31..0

    let mut frac_hi: u128 = (p_hi_top << 108) | (p5 << 76) | (p4 << 44) | (p3 << 12) | (p2 >> 20);
    let mut frac_lo: u128 = (p2 << 108) | (p1 << 76) | (p0 << 44);

    // 小数部が 0.5 以上なら象限を 1 進め、2 の補数で負の余りへ折り畳む。
    let mut frac_neg = false;
    if (frac_hi >> 127) != 0 {
        q = (q + 1) & 3;
        frac_neg = true;
        let (nlo, borrow) = 0u128.overflowing_sub(frac_lo);
        let nhi = 0u128.wrapping_sub(frac_hi).wrapping_sub(u128::from(borrow));
        frac_lo = nlo;
        frac_hi = nhi;
    }

    if frac_hi == 0 && frac_lo == 0 {
        let final_q = if neg { (-q) & 3 } else { q };
        return (final_q, 0.0, 0.0);
    }

    // 256-bit 余りを正規化し、最上位 128 bit を取り出す（最上位 bit = 1、値 = norm * 2^-(128 + shift)）。
    let (norm, shift) = if frac_hi != 0 {
        let lz = frac_hi.leading_zeros();
        let norm = if lz == 0 {
            frac_hi
        } else {
            (frac_hi << lz) | (frac_lo >> (128 - lz))
        };
        (norm, lz as i32)
    } else {
        let lz = frac_lo.leading_zeros();
        (frac_lo << lz, 128 + lz as i32)
    };

    // norm（Q0.128, 2^127 <= norm < 2^128）* PIO2_Q127（Q1.127, 2^127 <= PIO2 < 2^128）の上位 128 bit を求める。
    // 積の実際の値 = full_256 * 2^-(128 + shift + 127) = full_256 * 2^-(255 + shift)。
    let pio2: u128 = (u128::from(PIO2_Q127_HI) << 64) | u128::from(PIO2_Q127_LO);
    let (prod_hi, _prod_lo) = mul_u128_wide(norm, pio2);

    // prod_hi は full_256 >> 128 なので、実際の値 = prod_hi * 2^-(127 + shift)。
    // 最上位 bit の位置を揃えて、128-bit 正規化仮数 sig（bit 127 = 1）と指数 exp（値 = sig * 2^(exp - 127)）を得る。
    let lz2 = prod_hi.leading_zeros();
    let sig = prod_hi << lz2;
    let exp = -shift - (lz2 as i32);

    // sig（128 bit, bit 127 = 1）から上位 f64（y0）と下位 f64（y1）を構築する。
    let (y0_abs, y1_abs) = split_u128_to_f64_pair(sig, exp);

    let y_neg = neg ^ frac_neg;
    let y0 = if y_neg { -y0_abs } else { y0_abs };
    let y1 = if y_neg { -y1_abs } else { y1_abs };
    let final_q = if neg { (-q) & 3 } else { q };
    (final_q, y0, y1)
}

/// 2 つの `u128` の 256-bit 積 `(hi, lo)` を 64-bit 演算のみで計算する。
#[inline]
fn mul_u128_wide(a: u128, b: u128) -> (u128, u128) {
    let a_lo = a as u64;
    let a_hi = (a >> 64) as u64;
    let b_lo = b as u64;
    let b_hi = (b >> 64) as u64;

    let ll = u128::from(a_lo) * u128::from(b_lo);
    let lh = u128::from(a_lo) * u128::from(b_hi);
    let hl = u128::from(a_hi) * u128::from(b_lo);
    let hh = u128::from(a_hi) * u128::from(b_hi);

    let mid1 = (ll >> 64) + (lh & 0xffff_ffff_ffff_ffff) + (hl & 0xffff_ffff_ffff_ffff);
    let lo = (ll & 0xffff_ffff_ffff_ffff) | (mid1 << 64);
    let hi = hh + (lh >> 64) + (hl >> 64) + (mid1 >> 64);
    (hi, lo)
}

/// 正規化 128-bit 仮数 `sig`（bit 127 = 1）と 2 進指数 `exp`（値 = `sig * 2^(exp - 127)`）を、
/// `y0 + y1` の 2 つの非負 `f64` に分解する（`|y1| <= 0.5 ulp(y0)` となる最近接丸め）。
#[inline]
fn split_u128_to_f64_pair(sig: u128, exp: i32) -> (f64, f64) {
    // 上位 53 bit を取り出す（bit 127..75）。丸めビットは bit 74。
    let mut hi_mant = (sig >> 75) as u64; // 2^52 <= hi_mant < 2^53
    let rem_mask: u128 = (1u128 << 75) - 1;
    let mut rem = sig & rem_mask;
    let half: u128 = 1u128 << 74;

    let mut hi_exp = exp;
    let mut y1_neg = false;
    if rem > half || (rem == half && (hi_mant & 1) != 0) {
        hi_mant += 1;
        if hi_mant == (1u64 << 53) {
            hi_mant >>= 1;
            hi_exp += 1;
        }
        rem = (1u128 << 75) - rem;
        y1_neg = true;
    }

    let y0 = make_f64(false, hi_exp, hi_mant);
    if rem == 0 {
        return (y0, 0.0);
    }

    // 残り rem（75 bit 未満、値 = rem * 2^(exp - 127)）を正規化して y1 に変換する。
    let lz = rem.leading_zeros();
    let rem_norm = rem << lz; // bit 127 = 1
    let mut lo_exp = exp - (lz as i32);
    let mut lo_mant = (rem_norm >> 75) as u64;
    let lo_rem = rem_norm & rem_mask;
    if lo_rem > half || (lo_rem == half && (lo_mant & 1) != 0) {
        lo_mant += 1;
        if lo_mant == (1u64 << 53) {
            lo_mant >>= 1;
            lo_exp += 1;
        }
    }
    let y1 = make_f64(y1_neg, lo_exp, lo_mant);
    (y0, y1)
}

#[inline]
fn make_f64(neg: bool, exp: i32, mant53: u64) -> f64 {
    debug_assert!(((1u64 << 52)..(1u64 << 53)).contains(&mant53));
    let biased = exp + 1023;
    if biased <= 0 {
        // 非正規化数またはアンダーフロー（通常の余りでは十分大きいが安全に処理）
        let shift = (1 - biased) as u32;
        if shift >= 53 {
            return if neg { -0.0 } else { 0.0 };
        }
        let sub = mant53 >> shift;
        let bits = ((u64::from(neg)) << 63) | sub;
        return f64::from_bits(bits);
    }
    let bits =
        ((u64::from(neg)) << 63) | ((biased as u64) << 52) | (mant53 & 0x000f_ffff_ffff_ffff);
    f64::from_bits(bits)
}
