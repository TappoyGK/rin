# 演算・制限・エラーの仕様例

### eval-001 i64 と f64 の暗黙変換禁止

主根拠：`[type.numeric]`

期待規則：`[op.arith]`

**入力：**

```rin
(+ 1 1.0)
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 型エラー | `—` |

### eval-002 0 による除算は実行時エラー

主根拠：`[op.arith]`

期待規則：`[op.arith]`

**入力：**

```rin
(/ 10 0)
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 実行時エラー | `—` |

### eval-003 配列範囲外アクセスは実行時エラー

主根拠：`[expr.index]`

期待規則：`[expr.index]`

**入力：**

```rin
(let (xs [1 2])
  xs[2])
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 実行時エラー | `—` |

### eval-004 ステップ数上限超過の停止

主根拠：`[eval.limits]`

期待規則：`[eval.limits]`

**入力：**

```rin
(defn inf_loop ((n i64)) -> i64 (inf_loop (+ n 1)))
(inf_loop 0)
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| steps=50 | ステップ超過 | `—` |

### eval-005 再帰深さ上限超過の停止

主根拠：`[eval.limits]`

期待規則：`[eval.limits]`

**入力：**

```rin
(defn deep ((n i64)) -> i64 (+ 1 (deep (+ n 1))))
(deep 0)
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| depth=10 | 深さ超過 | `—` |
| 既定 | 深さ超過 | `—` |

### eval-006 アリーナメモリ上限超過の停止

主根拠：`[eval.limits]`

期待規則：`[eval.limits]`

**入力：**

```rin
(range 0 20)
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| arena=256 | メモリ超過 | `—` |

### eval-007 近いローカル名を候補として示す

主根拠：`[eval.hints]`

期待規則：`[expr.name]`

期待メッセージ：`unknown name 'damge'; did you mean 'damage'?`

**入力：**

```rin
(let (damage 10) damge)
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 型エラー | `—` |

### eval-008 フィールド一覧を名前順で示す

主根拠：`[eval.hints]`

期待規則：`[expr.field]`

期待メッセージ：`Point has no field 'bad'; fields: a, z`

**入力：**

```rin
(defschema Point (struct Point (z i64) (a i64)))
Point{z: 1 a: 2}.bad
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 型エラー | `—` |

### eval-009 スコープを抜けた名前は候補にしない

主根拠：`[eval.hints]`

期待規則：`[expr.name]`

期待メッセージ：`unknown name 'damge'`

**入力：**

```rin
(let (damage 10) 0) damge
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 型エラー | `—` |

### eval-010 ソース行とキャレットを表示する

主根拠：`[eval.snippet]`

期待規則：`[op.arith]`

**入力：**

```rin
(/ 1 0)
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 実行時エラー | `—` |

**期待表示：**

```text
1:1: division by zero [op.arith]
  |
1 | (/ 1 0)
  | ^
```

### eval-011 バイト列位置を全角文字の表示幅へ変換する

主根拠：`[eval.snippet]`

期待規則：`[expr.name]`

**入力：**

```rin
"猫" zzz
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 型エラー | `—` |

**期待表示：**

```text
1:7: unknown name 'zzz' [expr.name]
  |
1 | "猫" zzz
  |      ^
```

### eval-012 発生位置と複数の呼び出し位置の行番号欄を揃える

主根拠：`[eval.snippet]`

期待規則：`[op.arith]`

**入力：**

```rin
(defn f () -> i64 (/ 1 0))
(defn g () -> i64 (+ 1 (f)))







(g)
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 実行時エラー | `—` |

**期待表示：**

```text
1:19: division by zero [op.arith]
   |
 1 | (defn f () -> i64 (/ 1 0))
   |                   ^
  in f (at 2:24)
   |
 2 | (defn g () -> i64 (+ 1 (f)))
   |                        ^
  in g (at 10:1)
   |
10 | (g)
   | ^
```

### eval-013 組み込み関数から呼び戻す関数にも深さ上限を適用する

主根拠：`[eval.limits]`

期待規則：`[eval.limits]`

**入力：**

```rin
(defn deep ((n i64)) -> i64
  (fold (fn (acc x) (deep (+ n 1))) 0 [1]))
(deep 0)
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 深さ超過 | `—` |

### eval-014 決定性の対象となる数値演算

主根拠：`[eval.determinism]`

**入力：**

```rin
[(sqrt 4.0) (floor 1.5) (ceil -1.5) (abs -2.0)]
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `[2.0 1.0 -1.0 2.0]` |

### eval-015 同じ距離の候補名は名前順で表示する

主根拠：`[eval.determinism]`

期待規則：`[expr.name]`

期待メッセージ：`unknown name 'alphx'; did you mean 'alpha', 'alphi', 'alpho'?`

**入力：**

```rin
(let (alpho 3 alpha 1 alphi 2) alphx)
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 型エラー | `—` |

### eval-016 reverseの走査費用をステップに含める

主根拠：`[eval.builtin-cost]`

期待規則：`[eval.limits]`

**入力：**

```rin
(reverse [1 2 3])
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| steps=12 | ステップ超過 | `—` |

### eval-017 reverseの式評価・生成・走査が上限に収まる

主根拠：`[eval.builtin-cost]`

**入力：**

```rin
(reverse [1 2 3])
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| steps=13 | 値 | `[3 2 1]` |

### eval-018 sin・cosは環境によらず決定的な値を返す

主根拠：`[fn.math]`

**入力：**

```rin
[(sin 0.0) (cos 0.0) (sin (/ pi 2.0)) (cos pi)]
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `[0.0 1.0 1.0 -1.0]` |

### eval-019 同一環境での同名 def は実行時エラー

主根拠：`[expr.def]`

期待規則：`[expr.def]`

**入力：**

```rin
(def x 1)
(def x 2)
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 実行時エラー | `—` |

### eval-020 組み込み名の上書き（defn）は実行時エラー

主根拠：`[expr.defn]`

期待規則：`[expr.defn]`

**入力：**

```rin
(defn + ((a i64) (b i64)) -> i64 0)
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 実行時エラー | `—` |

### eval-021 let による大域束縛と組み込み名のシャドーイングは許可する

主根拠：`[expr.def]`

**入力：**

```rin
(def x 1)
(let (x 10 len 20) (+ x len))
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `30` |

### eval-022 eval_with の初期束縛はレキシカルスコープでのみ参照される

主根拠：`[env.isolation]`

初期束縛：`x i64 = 10`

**読み込み：**

```rin
(def x 1)
(defn f () -> i64 (+ x 1))
(defn invoke ((cb (fn () i64))) -> i64 (cb))
(defn make () -> (fn () i64) (fn () x))
```

**入力：**

```rin
(tuple x (f) (invoke (fn () x)) ((make)))
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `(tuple 10 2 10 1)` |

### eval-023 失敗した Builder::eval の追加束縛は取り消され再定義できる

主根拠：`[expr.def]`

**失敗する読み込み：**

```rin
(def a 1)
(/ 1 0)
```

**入力：**

```rin
(def a 2)
a
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `2` |

### eval-024 読み込み済みモジュール名と大域 def の衝突は実行時エラー

主根拠：`[expr.def]`

期待規則：`[expr.def]`

**モジュール `m`：**

```rin
(def v 1)
```

**入力：**

```rin
(def m 2)
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 実行時エラー | `—` |

### eval-025 datum の構築と型パターンによる取り出し

主根拠：`[type.datum]`

**入力：**

```rin
(match '1
  (i64(n) (+ n 1))
  (_ 0))
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `2` |

### eval-026 datum の暗黙受け入れと異種値の等値比較

主根拠：`[op.cmp]`

**入力：**

```rin
(tuple (== '1 1) (== '1 "1") (!= '1 "1"))
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `(tuple true false true)` |

### eval-027 datum の 8 種型パターン網羅

主根拠：`[expr.match]`

**入力：**

```rin
(defn kind ((d datum)) -> i64
  (match d
    (i64(_) 1)
    (f64(_) 2)
    (bool(_) 3)
    (char(_) 4)
    (text(_) 5)
    (ident(_) 6)
    (list(_) 7)
    (none 8)))
[(kind '1) (kind '(a b)) (kind 'none)]
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `[1 7 8]` |

### eval-028 any は型式として構文エラー

主根拠：`[type.expr]`

期待規則：`[type.expr]`

**入力：**

```rin
(defschema Bad (struct Bad (x any)))
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 構文エラー | `—` |

### eval-029 ホスト境界の型不一致は実行時エラー

主根拠：`[type.boundary]`

期待規則：`[type.boundary]`

初期束縛：`x i64 = "bad"`

**入力：**

```rin
(+ x 1)
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 実行時エラー | `—` |

### eval-030 他モジュールの構造体・バリアント値の構築と照合

主根拠：`[expr.struct]`

**モジュール `game`：**

```rin
(defschema Element (variant Element (Plain) (Fire) (Heat i64)))
(defschema HitResult (struct HitResult (dmg i64) (effect text)))
```

**入力：**

```rin
(import game)
(def h game.HitResult { dmg: 10 effect: "burn" })
(def e game.Element.Fire)
(def e2 game.Element.Heat(5))
(tuple
  h.dmg
  (match e (game.Element.Fire 1) (game.Element.Plain 0) (game.Element.Heat(n) n))
  (match e2 (game.Element.Fire 0) (game.Element.Plain 0) (game.Element.Heat(n) n)))
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `(tuple 10 1 5)` |

### eval-031 他モジュールの型名を値の位置に書くと型エラー

主根拠：`[expr.module]`

期待規則：`[expr.module]`

**モジュール `game`：**

```rin
(defschema HitResult (struct HitResult (dmg i64) (effect text)))
```

**入力：**

```rin
(import game)
game.HitResult
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 型エラー | `—` |
