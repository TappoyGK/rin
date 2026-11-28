# 式と構文の仕様例

### expr-001 defn による再帰定義と match 照合

主根拠：`[expr.defn]`

**入力：**

```rin
(defn fact ((n i64)) -> i64
  (if (<= n 1) 1 (* n (fact (- n 1)))))
(fact 5)
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `120` |

### expr-002 無名関数のレキシカル環境捕捉

主根拠：`[expr.fn]`

**入力：**

```rin
(let (base 10)
  (let (add_base (fn (x) (+ x base)))
    (add_base 5)))
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `15` |

### expr-003 if は第3引数が必須（省略すると構文エラー）

主根拠：`[expr.if]`

期待規則：`[expr.if]`

**入力：**

```rin
(if true 1)
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 構文エラー | `—` |

### expr-004 構造体の構築・フィールド参照・with による不変更新

主根拠：`[expr.with]`

**入力：**

```rin
(defschema Card (struct Card (id text) (cost i64)))
(let (c1 Card { id: "strike" cost: 1 })
  (let (c2 (with c1 { cost: 0 }))
    (+ c1.cost c2.cost)))
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `1` |

### expr-005 バリアント構築と match のペイロード束縛

主根拠：`[expr.variant]`

**入力：**

```rin
(defschema Effect (variant Effect (Gain i64) (Pass)))
(match Effect.Gain(7)
  (Effect.Gain(n) (+ n 3))
  (_ 0))
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `10` |

### expr-006 タプルリテラルの構築

主根拠：`[expr.tuple]`

**入力：**

```rin
(tuple 1 "ok" true)
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `(tuple 1 "ok" true)` |

### expr-007 通常の評価ではdeftestを拒否する

主根拠：`[test.deftest]`

期待規則：`[test.deftest]`

**入力：**

```rin
(deftest "example" (check true))
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 構文エラー | `—` |

### expr-008 未実行の枝でもcheckを拒否する

主根拠：`[test.check]`

期待規則：`[test.check]`

**入力：**

```rin
(if false (check true) 1)
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 構文エラー | `—` |

### expr-009 通常の評価ではcheck_eqを拒否する

主根拠：`[test.check]`

期待規則：`[test.check]`

**入力：**

```rin
(check_eq 1 1)
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 構文エラー | `—` |

### expr-010 構造体リテラルと with の重複フィールド名は最後の値が有効

主根拠：`[expr.struct]`

**入力：**

```rin
(defschema P (struct P (x i64)))
(tuple P { x: 1 x: 2 }.x (with P { x: 1 } { x: 2 x: 3 }).x)
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `(tuple 2 3)` |

### expr-011 quote と糖衣正規化

主根拠：`[expr.quote]`

**入力：**

```rin
'(+ 1 [a b] Point { x: 2 } p.x xs[0])
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `'(+ 1 (list a b) (struct Point (x 2)) (field p x) (index xs 0))` |

### expr-012 quasiquote の unquote と unquote-splicing

主根拠：`[expr.quasiquote]`

**入力：**

```rin
(let (x 10 xs [20 30])
  `(head ,x ,@xs tail))
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `'(head 10 20 30 tail)` |

### expr-013 テンプレート文字列の埋め込み評価

主根拠：`[expr.template]`

**入力：**

```rin
(let (x 1 y 2.0)
  "a${x}b${y}${'id}")
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `"a1b2.0id"` |

### expr-014 テンプレート文字列に埋め込めない型は型エラー

主根拠：`[expr.template]`

期待規則：`[expr.template]`

**入力：**

```rin
"${[1]}"
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 型エラー | `—` |

### expr-015 text の添字参照と slice

主根拠：`[expr.index]`

**入力：**

```rin
(tuple "日本"[1] (slice "日本語" 1 3))
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `(tuple \本 "本語")` |

### expr-016 複数行文字列の埋め込み式内の改行

主根拠：`[src.string]`

**入力：**

```rin
"""
  a${(+ 1
2)}b
  """
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `"a3b"` |

### expr-017 複数行文字列の埋め込み結果は加工しない

主根拠：`[expr.template]`

**入力：**

```rin
"""
  ${"x\n  y"}
  """
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `"x\n  y"` |
