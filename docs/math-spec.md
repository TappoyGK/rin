# 数学関数の決定的な計算手順

この文書は、Rin と Suz が共有する組み込み数学関数の計算手順の正本である。同じ入力と同じ FMA 設定（`Strict` / `Fused`）に対し、実装言語・コンパイラ・最適化設定・プラットフォームによらず、IEEE 754 binary64（`f64`）のビット列が完全に一致することを目指す。

テスト値は [sin-cos.tsv](math-vectors/sin-cos.tsv) に置く。手順または定数を変更する場合は、テスト値も同じ変更で更新し、[変更履歴](changelog.md) に記載する。

## 1. 共通の前提

- 浮動小数点数は IEEE 754 binary64、丸めモードは最近接・偶数丸め（round-to-nearest-even）とする。
- `Strict` 設定では、四則演算（`+`・`-`・`*`・`/`）を単独の演算として評価し、積和融合（FMA）を行わない。C 実装では `-ffp-contract=off`（および `#pragma STDC FP_CONTRACT OFF`）を必須とする。
- `Fused` 設定では、本文書で `fma(a, b, c)` と明記した位置に限り、IEEE 754 `fusedMultiplyAdd`（Rust の `f64::mul_add`、C の `fma`）を使う。それ以外の乗算・加算は融合しない。
- NaN の符号ビットとペイロードは一致の保証対象外とし、NaN を返す入力では任意の quiet NaN を返す（以下では `x - x` で生成する）。NaN 以外の値は符号付きゼロを含めてビット単位で一致させる。
- 動的メモリ確保は行わず、すべての処理は引数の大きさによらない定数上限（O(1)）で完了する。

## 2. `sin`・`cos`

### 2.1 精度と特殊値

- 精度上限：`Strict`・`Fused` ともに、すべての有限 `f64` 入力に対して真の値との差は **1 ulp 以内** とする。
- 特殊値：
  - `sin(NaN) = NaN`、`cos(NaN) = NaN`
  - `sin(±inf) = NaN`、`cos(±inf) = NaN`
  - `sin(+0.0) = +0.0`、`sin(-0.0) = -0.0`
  - `cos(+0.0) = +1.0`、`cos(-0.0) = +1.0`

### 2.2 定数（IEEE 754 binary64 ビット列）

カーネル多項式係数：

| 定数 | 16 進ビット列 |
|---|---|
| `S1` | `0xbfc5555555555549` |
| `S2` | `0x3f8111111110f8a6` |
| `S3` | `0xbf2a01a019c161d5` |
| `S4` | `0x3ec71de357b1fe7d` |
| `S5` | `0xbe5ae5e68a2b9ceb` |
| `S6` | `0x3de5d93a5acfd57c` |
| `C1` | `0x3fa555555555554c` |
| `C2` | `0xbf56c16c16c15177` |
| `C3` | `0x3efa01a019cb1590` |
| `C4` | `0xbe927e4f809c52ad` |
| `C5` | `0x3e21ee9ebdb4b1c4` |
| `C6` | `0xbda8fae9be8838d4` |

Cody–Waite 3 段縮約定数：

| 定数 | 16 進ビット列 |
|---|---|
| `INV_PIO2` | `0x3fe45f306dc9c883` |
| `PIO2_1` | `0x3ff921fb54400000` |
| `PIO2_1T` | `0x3dd0b4611a626331` |
| `PIO2_2` | `0x3dd0b4611a600000` |
| `PIO2_2T` | `0x3ba3198a2e037073` |
| `PIO2_3` | `0x3ba3198a2e000000` |
| `PIO2_3T` | `0x397b839a252049c1` |

Payne–Hanek 縮約用定数：

- `TWO_OVER_PI[44]`（`u32` 配列、先頭 2 ワードは `0`、以降は `2/π` の 2 進小数展開を 32 ビットごとにビッグエンディアン順で格納）：

```text
0x00000000, 0x00000000,
0xa2f9836e, 0x4e441529, 0xfc2757d1, 0xf534ddc0,
0xdb629599, 0x3c439041, 0xfe5163ab, 0xdebbc561,
0xb7246e3a, 0x424dd2e0, 0x06492eea, 0x09d1921c,
0xfe1deb1c, 0xb129a73e, 0xe88235f5, 0x2ebb4484,
0xe99c7026, 0xb45f7e41, 0x3991d639, 0x835339f4,
0x9c845f8b, 0xbdf9283b, 0x1ff897ff, 0xde05980f,
0xef2f118b, 0x5a0a6d1f, 0x6d367ecf, 0x27cb09b7,
0x4f463f66, 0x9e5fea2d, 0x7527bac7, 0xebe5f17b,
0x3d0739f7, 0x8a5292ea, 0x6bfb5fb1, 0x1f8d5d08,
0x56033046, 0xfc7b6bab, 0xf0cfbc20, 0x9af4361d,
0xa9e39161, 0x5ee61b08
```

- `PIO2_Q127`（`π/2` を `2^127` 倍して最近接丸めした 128 ビット符号なし整数）：
  - `PIO2_Q127_HI = 0xc90fdaa22168c234`
  - `PIO2_Q127_LO = 0xc4c6628b80dc1cd1`

### 2.3 入口の分岐

入力 `x` の上位 32 ビットから符号を除いた値を `ix = ((bits(x) >> 32) & 0x7fffffff)` とする。

- `sin(x)`：
  1. `ix <= 0x3fe921fb`（`|x| <= π/4`）：
     - `ix < 0x3e500000`（`|x| < 2^-26`）なら `x` をそのまま返す。
     - それ以外は `kernel_sin(x, 0.0, false)` を返す。
  2. `ix >= 0x7ff00000`（NaN または `±inf`）：`x - x` を返す。
  3. それ以外：`(n, y0, y1) = rem_pio2(x)` を求め、`n & 3` に応じて次を返す。
     - `0`: `kernel_sin(y0, y1, true)`
     - `1`: `kernel_cos(y0, y1)`
     - `2`: `-kernel_sin(y0, y1, true)`
     - `3`: `-kernel_cos(y0, y1)`

- `cos(x)`：
  1. `ix <= 0x3fe921fb`（`|x| <= π/4`）：
     - `ix < 0x3e400000`（`|x| < 2^-27`）なら `1.0` を返す。
     - それ以外は `kernel_cos(x, 0.0)` を返す。
  2. `ix >= 0x7ff00000`（NaN または `±inf`）：`x - x` を返す。
  3. それ以外：`(n, y0, y1) = rem_pio2(x)` を求め、`n & 3` に応じて次を返す。
     - `0`: `kernel_cos(y0, y1)`
     - `1`: `-kernel_sin(y0, y1, true)`
     - `2`: `-kernel_cos(y0, y1)`
     - `3`: `kernel_sin(y0, y1, true)`

### 2.4 カーネル関数（`kernel_sin`・`kernel_cos`）

共通の前計算：`z = x * x`、`w = z * z`、`v = z * x`（`kernel_sin`）、`hz = 0.5 * z`、`w2 = 1.0 - hz`（`kernel_cos`）。

- `kernel_sin(x, y, iy)`：
  - `Strict`：
    1. `r = S2 + z * (S3 + z * S4) + z * w * (S5 + z * S6)`（左から `(S2 + (z * (S3 + (z * S4)))) + ((z * w) * (S5 + (z * S6)))`）
    2. `iy == false` なら `x + v * (S1 + z * r)`
    3. `iy == true` なら `x - ((z * (0.5 * y - v * r) - y) - v * S1)`
  - `Fused`：
    1. `r1 = fma(z, S4, S3)`
    2. `r2 = fma(z, r1, S2)`
    3. `r3 = fma(z, S6, S5)`
    4. `r = fma(z * w, r3, r2)`
    5. `iy == false` なら `fma(v, fma(z, r, S1), x)`
    6. `iy == true` なら `t = fma(v, -r, 0.5 * y)`、`u = fma(z, t, -y)`、`x - fma(v, -S1, u)`

- `kernel_cos(x, y)`：
  - `Strict`：
    1. `r = z * (C1 + z * (C2 + z * C3)) + w * w * (C4 + z * (C5 + z * C6))`
    2. `w2 + (((1.0 - w2) - hz) + (z * r - x * y))`
  - `Fused`：
    1. `p1 = fma(z, fma(z, C3, C2), C1)`
    2. `p2 = fma(z, fma(z, C6, C5), C4)`
    3. `r = fma(w * w, p2, z * p1)`
    4. `corr = fma(z, r, -(x * y))`
    5. `w2 + (((1.0 - w2) - hz) + corr)`

### 2.5 引数縮約（`rem_pio2`）

`rem_pio2(x)` は `Strict`・`Fused` で同一の手順とし、FMA は使わない。`neg = (bits(x) >> 63) != 0` とする。

1. **経路 1（`ix < 0x4002d97c`、`π/4 < |x| < 3π/4`）**：
   - `neg == false`：
     - `z = x - PIO2_1`
     - `ix != 0x3ff921fb` なら `y0 = z - PIO2_1T`、`y1 = (z - y0) - PIO2_1T`
     - そうでなければ `z = z - PIO2_2`、`y0 = z - PIO2_2T`、`y1 = (z - y0) - PIO2_2T`
     - `(1, y0, y1)` を返す。
   - `neg == true`：
     - `z = x + PIO2_1`
     - `ix != 0x3ff921fb` なら `y0 = z + PIO2_1T`、`y1 = (z - y0) + PIO2_1T`
     - そうでなければ `z = z + PIO2_2`、`y0 = z + PIO2_2T`、`y1 = (z - y0) + PIO2_2T`
     - `(-1, y0, y1)` を返す。

2. **経路 2（`ix <= 0x413921fb`、`3π/4 <= |x| <= 2^19 * π/2`、Cody–Waite 3 段縮約）**：
   - `t = |x|`、`n = (i32)(t * INV_PIO2 + 0.5)`（0 方向切り捨て）、`fn = (f64)n`
   - `r = t - fn * PIO2_1`、`w = fn * PIO2_1T`、`y0 = r - w`
   - `i = (i32)ix - (i32)((bits(y0) >> 32) & 0x7ff00000)`
   - `i > (16 << 20)` なら：
     - `t2 = r`、`w = fn * PIO2_2`、`r = t2 - w`、`w = fn * PIO2_2T - ((t2 - r) - w)`、`y0 = r - w`
     - `i = (i32)ix - (i32)((bits(y0) >> 32) & 0x7ff00000)`
     - `i > (49 << 20)` なら：
       - `t3 = r`、`w = fn * PIO2_3`、`r = t3 - w`、`w = fn * PIO2_3T - ((t3 - r) - w)`、`y0 = r - w`
   - `y1 = (r - y0) - w`
   - `neg == true` なら `(-n, -y0, -y1)`、そうでなければ `(n, y0, y1)` を返す。

3. **経路 3（`ix > 0x413921fb`、`|x| > 2^19 * π/2`、整数 Payne–Hanek 縮約）**：
   - `e = ((bits(x) >> 52) & 0x7ff) - 1023`（`19 <= e <= 1023`）
   - `m = (bits(x) & 0x000fffffffffffff) | 0x0010000000000000`（53 ビット符号なし整数）
   - `idx = e / 32`、`rem = e % 32`
   - `j = 0..6` について、32 ビットワード `w[j]` を取り出す：
     - `rem == 0` なら `w[j] = TWO_OVER_PI[idx + j]`
     - `rem != 0` なら `w[j] = (TWO_OVER_PI[idx + j] << rem) | (TWO_OVER_PI[idx + j + 1] >> (32 - rem))`
   - 224 ビット符号なし整数 `W = Σ_{j=0..6} w[j] * 2^(32*(6-j))` と `m` の積 `P = m * W` を 32 ビット×9 ワードのリトルエンディアン配列 `prod[0..8]` として計算する。
   - 象限 `q = (prod[6] >> 20) & 3`。
   - 小数部 212 ビット（`P` の bit 211..0）を 256 ビット符号なし整数 `(frac_hi, frac_lo)` の上位へ 44 ビット左詰めして格納する：
     - `frac_hi = ((prod[6] & 0xfffff) << 108) | (prod[5] << 76) | (prod[4] << 44) | (prod[3] << 12) | (prod[2] >> 20)`
     - `frac_lo = (prod[2] << 108) | (prod[1] << 76) | (prod[0] << 44)`
   - `(frac_hi >> 127) != 0` なら `q = (q + 1) & 3`、`frac_neg = true`、`(frac_hi, frac_lo) = - (frac_hi, frac_lo)`（256 ビット 2 の補数）。そうでなければ `frac_neg = false`。
   - `frac_hi == 0 && frac_lo == 0` なら `(neg ? (-q) & 3 : q, 0.0, 0.0)` を返す。
   - `(frac_hi, frac_lo)` を最上位 1 ビットが `norm` の bit 127 に来るよう左シフトして 128 ビット値 `norm` とシフト量 `shift` を得る。
   - 128 ビット×128 ビットの完全積 `norm * PIO2_Q127` の上位 128 ビット `prod_hi` を求め、先頭ゼロ数 `lz2` だけ左シフトして `sig = prod_hi << lz2`、`exp = -shift - lz2` とする。
   - `sig`（値 = `sig * 2^(exp - 127)`）の上位 53 ビット（bit 127..75）を最近接・偶数丸めして `y0_abs` を作り、残差（bit 74..0、切り上げ時は `2^75 - rem` で負符号）を同様に正規化・最近接・偶数丸めして `y1_abs` を作る。
   - `y_neg = neg ^ frac_neg` とし、`y0 = y_neg ? -y0_abs : y0_abs`、`y1 = y_neg ? -y1_abs : y1_abs`、`final_q = neg ? (-q) & 3 : q` として `(final_q, y0, y1)` を返す。

## 3. テスト値の形式

[sin-cos.tsv](math-vectors/sin-cos.tsv) は UTF-8 のタブ区切りテキストである。`#` で始まる行と空行はコメント・区切りとして無視する。各データ行は次の 5 列からなる。

```text
func<TAB>input_hex<TAB>strict_hex<TAB>fused_hex<TAB>note
```

- `func`: `sin` または `cos`
- `input_hex`: 入力の `f64` ビット列（`0x` 付き 16 桁小文字 16 進数）
- `strict_hex`・`fused_hex`: 期待結果の `f64` ビット列、または任意の quiet NaN を表す `NaN`
- `note`: テストケースの説明
