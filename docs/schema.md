# Rin スキーマ仕様

Rin の値（`Value`）がホスト側の期待する形に適合するかを検証するための、軽量なスキーマ専用言語（Rin Schema DSL）を定める。

## 1. 構成

プログラムは 0 個以上の名前付きスキーマ定義 `(defschema Name TypeExpr)` と、末尾の 1 つのルート型式 `TypeExpr` からなる。

```rin
(defschema Element (variant Element (Plain) (Fire) (Frost)))
(defschema Card (struct Card (id string) (cost i64) (element Element)))
(list Card)
```

## 2. 型式（`TypeExpr`）

### 2.1 プリミティブ・ワイルドカード

- `void`、`i64`、`f64`、`bool`、`char`、`string`
- `any`：任意の型・値を受け入れる。

### 2.2 コンテナ・合成型

- `(list T)`：全要素が `T` に適合する `list`
- `(array T)`：全要素が `T` に適合する `array`
- `(tuple T1 T2 ...)`：要素数が一致し、各要素が `T1, T2, ...` に適合する `tuple`（2 要素以上）
- `(option T)`：`none`、または値が `T` に適合する `some(v)`
- `(result T E)`：値が `T` に適合する `ok(v)`、または `E` に適合する `err(e)`

### 2.3 構造体（厳格・開放の選択）

- `(struct Name (f1 T1) (f2 T2) ...)`（厳格）：構造体名が `Name` に一致し、定義された全フィールドが揃い、**かつ未定義の余剰フィールドが存在しない**こと。
- `(open_struct Name (f1 T1) (f2 T2) ...)`（開放）：構造体名が `Name` に一致し、定義された全フィールドが存在して型が適合すること（**余剰フィールドは許容する**）。

### 2.4 バリアント・名前参照

- `(variant Name (V1) (V2 T) ...)`：バリアント型名が `Name` に一致し、バリアント名およびペイロードの有無・型が一致すること。
- `Name`：`defschema` で定義されたスキーマを参照する（自己再帰・相互参照を許す）。
