# リストと配列の仕様例

### list-001 cons・car・cdr による再帰マップ

主根拠：`[fn.list]`

**入力：**

```rin
(defn double_all (xs)
  (match (car xs)
    (none [])
    (some(h) (cons (* h 2) (double_all (cdr xs))))))
(double_all (list 1 2 3))
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `[2 4 6]` |

### list-002 append・reverse・take・drop・slice

主根拠：`[fn.list]`

**入力：**

```rin
(slice (reverse (append [1 2] [3 4 5])) 1 4)
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `[4 3 2]` |

### list-003 filter と fold による集計

主根拠：`[fn.list]`

**入力：**

```rin
(let (evens (filter (fn (x) (== (% x 2) 0)) (range 0 6)))
  (fold + 0 evens))
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `6` |

### list-004 flat_map・sort_by・unique

主根拠：`[fn.list]`

**入力：**

```rin
(sort_by (fn (x) (- 0 x)) (unique (flat_map (fn (x) [x x]) [1 3 2])))
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `[3 2 1]` |

### list-005 zip と enumerate

主根拠：`[fn.list]`

**入力：**

```rin
(zip (range 0 2) ["a" "b" "c"])
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `[(tuple 0 "a") (tuple 1 "b")]` |

### list-006 list->array と array->list の相互変換

主根拠：`[fn.list]`

**入力：**

```rin
(let (arr (list->array [10 20 30]))
  (cons arr[1] (array->list arr)))
```

**期待結果：**

| 上限 | 結果 | 値 |
|---|---|---|
| 既定 | 値 | `[20 10 20 30]` |
