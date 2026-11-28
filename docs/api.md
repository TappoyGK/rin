# Rin API 仕様

Rin をホストプログラム（Rust および C / FFI）から利用するための API を定める。

---

## 1. Rust API

クレート名：`rin`

### 1.1 簡易評価関数（`std-value` フィーチャ）

> 既定で有効な Cargo フィーチャ `std-value` を前提とする便利 API である（システムヒープ上に所有型 `Value` / `Error` を構築する）。`--no-default-features` でゼロヒープ構成にする場合は 1.2.1 の `Val` / `ValueRef` / `ErrorRef` API を使用する。

```rust
pub fn eval(source: &str) -> Result<Value, Error>
pub fn eval_with(source: &str, bindings: &[(&str, &str, Value)], limits: Limits) -> Result<Value, Error>
pub fn eval_with_allocator(
    source: &str,
    bindings: &[(&str, &str, Value)],
    limits: Limits,
    alloc: &mut dyn Allocator,
) -> Result<Value, Error>
```

- `eval`：`Limits::default()`・`HeapAllocator` で式列を評価する。
- `eval_with`：型式付きの初期束縛 `bindings`（`(name, type_expr, value)`）と資源上限 `limits` を指定して評価する。
- `eval_with_allocator`：呼び出し元が指定した `Allocator` 上で評価する。
- これら 3 つの簡易評価関数は、読み込み段階の環境で 1 回評価し、数学関数の FMA 設定として常に `MathFma::Strict` を使う。凍結した実行環境や `MathFma::Fused` を使う場合は `Builder` と `Machine` を利用する。

### 1.2 読み込み環境 `Builder<'a>` と凍結環境 `Machine<'a>`

大域環境とモジュールの構築（読み込みの段階）は `Builder<'a>` で行い、`into_machine`（または `std-value` 有効時の `freeze`）によって実行専用の `Machine<'a>`（実行の段階）へ移行する。

```rust
impl<'a> Builder<'a> {
    // 常に利用可能（ゼロヒープ）
    pub fn new(limits: Limits, alloc: &'a mut dyn Allocator) -> Self
    pub fn with_memory_config(limits: Limits, memory: MemoryConfig, alloc: &'a mut dyn Allocator) -> Self
    pub fn set_math_fma(&mut self, mode: MathFma)
    pub fn math_fma(&self) -> MathFma
    pub fn live_bytes(&self) -> usize
    pub fn into_machine(self) -> Machine<'a>

    // std-value フィーチャ有効時（所有型 Value / Error を使用）
    pub fn bind(&mut self, name: &str, type_expr: &str, value: &Value) -> Result<(), Error>
    pub fn load_module(&mut self, name: &str, source: &str) -> Result<(), Error>
    pub fn eval(&mut self, source: &str) -> Result<Value, Error>
    pub fn freeze(self) -> Result<Machine<'a>, Error>
}

impl<'a> Machine<'a> {
    // 常に利用可能（ゼロヒープ）
    pub fn set_math_fma(&mut self, mode: MathFma)
    pub fn math_fma(&self) -> MathFma
    pub fn live_bytes(&self) -> usize

    // std-value フィーチャ有効時（所有型 Value / Error を使用）
    pub fn eval(&mut self, source: &str) -> Result<Value, Error>
    pub fn eval_with(&mut self, source: &str, bindings: &[(&str, &str, Value)]) -> Result<Value, Error>
    pub fn call(&mut self, fn_name: &str, args: &[Value]) -> Result<Value, Error>
    pub fn call_in(&mut self, mod_name: &str, fn_name: &str, args: &[Value]) -> Result<Value, Error>
}
```

- `Builder::new` / `Builder::with_memory_config`：読み込み段階の環境を生成する。`with_memory_config` で 3 領域（永続・オブジェクト・作業）のチャンクサイズ設定 `MemoryConfig` を指定できる。
- `Builder::bind`：ホストの `Value` を内部プールに取り込み、大域変数 `name` として束縛する。既存の大域束縛・組み込み名・読み込み済みモジュール名と衝突する場合は実行時エラー（`[expr.def]`、位置 `0:0`）を返す。
- `Builder::load_module`：名前付きモジュールを読み込んで評価する。読み込み済みのモジュール名、または既存の大域束縛・組み込み名と衝突する場合は実行時エラー（`[expr.module]`、位置 `0:0`）を返す。読み込みに失敗したモジュールは登録されず、同じ名前で再試行できる。
- `Builder::eval`：読み込み段階でソースを評価する（トップレベルの `def`・`defn`・`import` 等を許可する）。途中でエラーになった場合、その評価中に追加した大域束縛・`import` は追加のメモリ確保なしで取り消される。
- **永続領域の追記特性**：読み込み段階（`Builder`）の評価では、初めて現れる大域名とモジュール名（定義か参照かを問わず、エラーになった評価も含む）、構造体リテラルに初めて現れる構造体名・フィールド名、および関数リテラル（`fn` / `defn`）のたびに永続領域（`PermArena`）へ追記され、`Builder` / `Machine` が破棄されるまで回収されない（失敗した評価で追加された大域束縛の値スロットは取り消し時に解放される）。反復評価は凍結後の `Machine` で行い、ホットリロード時は新しい `Builder` を作り直すこと。
- `Builder::into_machine` / `Builder::freeze`：大域環境と読み込み済みモジュール、および永続領域（`PermArena`）を凍結し、実行段階の `Machine` を返す。`into_machine` は失敗しないゼロヒープ API、`freeze` は `std-value` フィーチャ有効時の `Result` ラッパー（現行実装では常に `Ok` を返す）。
- 凍結後の `Machine` から読み込みの段階には戻れない。Mod を読み直す場合は、新しい `Builder` を作って読み込み・凍結し、ホスト側で `Machine` を差し替える（ホットリロードは組み込む側が担う）。
- `Machine::eval` / `Machine::eval_with`：凍結した環境でソースを評価する。`def`・`defn`・`defschema`・`import`・`export` を含むソースは評価前に `[env.frozen]` の構文エラーとする。`eval_with` の `bindings`（初期束縛）はその 1 回の評価にだけ有効な最外側のレキシカル束縛であり、評価するソースおよびそのソース内で生成されたクロージャからのみ参照できる（凍結済みの大域関数・モジュール内関数やそれらが返すクロージャからは参照されない）。初期束縛は評価終了時に解放され、`bindings` に同じ名前が重複した場合は後ろの束縛が有効になる。読み込み済みのモジュールは `import` なしで `mod_name.symbol` として参照できる。評価するソース内の名前解決の優先順位は、内側の局所変数・捕捉変数 → 初期束縛 → 凍結された大域束縛（組み込みを含む） → 読み込み済みモジュール名とする。
- Rust API では `call` / `call_in` は凍結後の `Machine` のみ提供する（C FFI では同一ハンドル上で `rin_call` / `rin_call_in` を凍結前・凍結後のどちらでも呼び出せる）。
- `set_math_fma` / `math_fma`：組み込み数学関数（`sin`・`cos`）の FMA 使用方針（`MathFma::Strict` / `MathFma::Fused`、既定は `Strict`）を設定・取得する。
- `live_bytes`：現在オブジェクト領域上で生存している論理使用バイト数を返す。

### 1.2.1 ゼロヒープ・ホスト境界 API（`Val` / `ValueRef` / `ErrorRef`）

`Builder` / `Machine` は、Rust のシステムヒープを一切使わず呼び出し元が渡した `Allocator` の上だけで完結する API を提供する（Cargo フィーチャ `std-value` を無効化した `--no-default-features` でもすべて利用可能）。

```rust
// 値ハンドルの構築・解放（Builder / Machine 共通）
pub fn val_void(&self) -> Val
pub fn val_int(&self, n: i64) -> Val
pub fn val_float(&self, x: f64) -> Val
pub fn val_bool(&self, b: bool) -> Val
pub fn val_char(&self, c: char) -> Val
pub fn val_none(&self) -> Val
pub fn val_text(&mut self, s: &str) -> Result<Val, ErrorRef<'_>>
pub fn val_ident(&mut self, s: &str) -> Result<Val, ErrorRef<'_>>
pub fn val_list(&mut self, items: &[Val]) -> Result<Val, ErrorRef<'_>>
pub fn val_array(&mut self, items: &[Val]) -> Result<Val, ErrorRef<'_>>
pub fn val_tuple(&mut self, items: &[Val]) -> Result<Val, ErrorRef<'_>>
pub fn val_struct(&mut self, name: &str, fields: &[(&str, Val)]) -> Result<Val, ErrorRef<'_>>
pub fn val_variant(&mut self, type_name: &str, variant: &str, data: Option<Val>) -> Result<Val, ErrorRef<'_>>
pub fn val_some(&mut self, v: Val) -> Result<Val, ErrorRef<'_>>
pub fn val_ok(&mut self, v: Val) -> Result<Val, ErrorRef<'_>>
pub fn val_err(&mut self, v: Val) -> Result<Val, ErrorRef<'_>>
pub fn release_val(&mut self, v: Val)
pub fn value_ref(&self, v: Val) -> ValueRef<'_>

// Builder
pub fn resolve_type(&mut self, type_expr: &str) -> Result<TypeHandle, ErrorRef<'_>>
pub fn bind_val(&mut self, name: &str, type_expr: &str, val: Val) -> Result<(), ErrorRef<'_>>
pub fn bind_typed(&mut self, name: &str, ty: TypeHandle, val: Val) -> Result<(), ErrorRef<'_>>
pub fn load_module_ref(&mut self, name: &str, source: &str) -> Result<(), ErrorRef<'_>>
pub fn eval_ref(&mut self, source: &str) -> Result<ValueRef<'_>, ErrorRef<'_>>
pub fn fn_sig(&self, fn_name: &str) -> Option<FnSigRef<'_>>
pub fn fn_sig_in(&self, mod_name: &str, fn_name: &str) -> Option<FnSigRef<'_>>
pub fn type_of(&self, name: &str) -> Option<TypeRef<'_>>
pub fn type_of_in(&self, mod_name: &str, name: &str) -> Option<TypeRef<'_>>

// Machine
pub fn eval_ref(&mut self, source: &str) -> Result<ValueRef<'_>, ErrorRef<'_>>
pub fn eval_with_vals(&mut self, source: &str, bindings: &[(&str, &str, Val)]) -> Result<ValueRef<'_>, ErrorRef<'_>>
pub fn eval_with_typed_vals(&mut self, source: &str, bindings: &[(&str, TypeHandle, Val)]) -> Result<ValueRef<'_>, ErrorRef<'_>>
pub fn call_ref(&mut self, fn_name: &str, args: &[Val]) -> Result<ValueRef<'_>, ErrorRef<'_>>
pub fn call_in_ref(&mut self, mod_name: &str, fn_name: &str, args: &[Val]) -> Result<ValueRef<'_>, ErrorRef<'_>>
pub fn fn_sig(&self, fn_name: &str) -> Option<FnSigRef<'_>>
pub fn fn_sig_in(&self, mod_name: &str, fn_name: &str) -> Option<FnSigRef<'_>>
pub fn type_of(&self, name: &str) -> Option<TypeRef<'_>>
pub fn type_of_in(&self, mod_name: &str, name: &str) -> Option<TypeRef<'_>>
```

- **`Val` の所有権規約**：
  - `Val` はマシン所有のプール値への不透明ハンドル（`Copy` だが参照カウント管理）。スカラー（`val_void` / `val_int` / `val_float` / `val_bool` / `val_char` / `val_none`）はプールを確保しないが、ヒープオブジェクトを伴う値（`val_text` / `val_ident` / `val_list` / `val_array` / `val_tuple` / `val_struct` / `val_variant` / `val_some` / `val_ok` / `val_err`）は呼び出し元が 1 参照を所有する。
  - コンテナ構築（`val_list` / `val_array` / `val_tuple` / `val_struct` / `val_variant` / `val_some` / `val_ok` / `val_err`）および `bind_val` / `call_ref` / `call_in_ref` / `eval_with_vals` は渡された `Val` を内部で retain して共有参照する。**所有権は移動しない**ため、呼び出し元は自分が生成した `Val` を使い終わったタイミングで必ず同じマシンの `release_val` へ返却する。
  - 解放済みの `Val` の再利用、二重 `release_val`、異なる `Builder` / `Machine` で生成した `Val` の受け渡しは不正であり、パニックするか別の値を読む。特に、解放済みの `Val` をスロット再利用後にもう一度 `release_val` へ渡した場合や、別の `Machine` の `Val`（`ObjId` が範囲内のもの）を `release_val` へ渡した場合は、ほかのハンドルや束縛が保持している別の値を解放し、それらが再利用後の別値を指すようになることがある（どの場合も境界検査によりメモリの範囲外には触れない）。
- **`ValueRef<'_>` と補助借用ビュー**：
  - プール上の値を複製せずに直接読み取る借用ビュー。マシンの可変参照の借用期間中（次の可変操作まで）有効。
  - 公開型：`ValueTag`（`Void` / `Int` / `Float` / `Bool` / `Char` / `Text` / `Ident` / `List` / `Array` / `Tuple` / `Struct` / `Variant` / `None` / `Some` / `Ok` / `Err` / `Fn`）、`ListIter<'a>`（`Iterator<Item = ValueRef<'a>>`）、`SliceView<'a>`（`len` / `is_empty` / `get` / `iter`）、`StructRef<'a>`（`name` / `len` / `is_empty` / `field` / `get` / `fields`）、`VariantRef<'a>`（`type_name` / `variant` / `data`）。
  - 主なメソッド：`tag`・`is_void`・`is_none`・`as_int`・`as_float`・`as_bool`・`as_char`・`as_text`・`as_ident`・`as_list`（`Option<ListIter<'_>>`）・`list_len`（`Option<usize>`）・`as_array`（`Option<SliceView<'_>>`）・`as_tuple`（`Option<SliceView<'_>>`）・`as_struct`（`Option<StructRef<'_>>`）・`as_variant`（`Option<VariantRef<'_>>`）・`as_some`・`as_ok`・`as_err`。
  - `Display` もシステムヒープを使わず `fmt::Write` へ直接出力する。`std-value` フィーチャ有効時は `to_value(&self) -> Value` で所有値へ変換できる。
- **`ErrorRef<'_>`**：
  - マシン内のインラインエラーストアを参照する借用エラービュー（`kind`・`at`・`rule`・`message`・`module`・`backtrace_len`・`omitted_frames`・`write_backtrace`）。次の可変操作まで有効。`Display` を含めシステムヒープを一切使わない。`std-value` フィーチャ有効時は `to_error(&self) -> Error` で所有型 `Error` へ変換できる。

### 数学関数の直接呼び出しと `MathFma`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MathFma {
    #[default]
    Strict,
    Fused,
}

pub fn rin::math::sin(x: f64, mode: MathFma) -> f64
pub fn rin::math::cos(x: f64, mode: MathFma) -> f64
```

- `MathFma::Strict`：FMA を使わず IEEE 754 の四則演算のみで計算する。
- `MathFma::Fused`：[数学関数の決定的な計算手順](math-spec.md) で定めた位置に限り `f64::mul_add` を使う。
- `rin::math::sin` / `rin::math::cos` は Rin 評価器が用いる決定的な実装を直接呼び出す。

### 1.3 カスタムアロケータ `Allocator` と領域設定 `MemoryConfig`

```rust
pub unsafe trait Allocator {
    fn alloc(&mut self, layout: Layout) -> Option<NonNull<u8>>;
    unsafe fn dealloc(&mut self, ptr: NonNull<u8>, layout: Layout);
}

pub struct RegionConfig {
    pub initial_block: usize, // 既定: 4 * 1024（内部最小値 256）
    pub max_block: usize,     // 既定: 64 * 1024
}

pub struct MemoryConfig {
    pub perm: RegionConfig, // 永続領域
    pub obj: RegionConfig,  // オブジェクト領域
    pub work: RegionConfig, // 作業領域
}
```

`Allocator` の安全性契約（`unsafe impl Allocator` の実装側および呼び出し側）：
- **実装側の契約**：
  - `alloc(layout)` が `Some(ptr)` を返すとき、`layout.size() > 0` であれば `ptr` は `layout.align()` に整列した少なくとも `layout.size()` バイトの読み書き可能な有効領域を指すこと。
  - 同時に生存している（未解放の）確保領域どうしを重複させず、対応する `dealloc(ptr, layout)` またはアロケータ自体の破棄まで有効に保つこと。アリーナ実装で個別解放を省略する場合も、生存中の領域を早期に再利用してはならない。
  - `dealloc(ptr, layout)` は、対応する確保方式と `layout` に従って安全に解放（アリーナの場合は無操作）すること。
  - `layout.size() == 0` の要求に対しては、成功時は `layout.align()` に整列した非 NULL ポインタを返し、`dealloc` は何もせず安全に戻ること（組み込みの `HeapAllocator`・`ArenaAllocator` および C アロケータアダプタはこの契約に従う）。
- **呼び出し側の契約**：
  - `dealloc(ptr, layout)` には `self.alloc(layout)` が返した未解放のポインタと、確保時と同じ `layout` のみを渡すこと。
  - `layout.size() == 0` で取得したポインタを通じて読み書きしないこと。

組み込み実装：
- `HeapAllocator`：Rust 標準ヒープを使うデフォルト実装。
- `ArenaAllocator<'a>`：`ArenaAllocator::new(buf: &'a mut [u8])` で渡したバイト列をバンプ確保する実装（`dealloc` は何もしない）。`used()` / `capacity()` で使用量を確認できる。
- `MemoryConfig::default()` は 3 領域すべて `initial_block = 4 KiB`、`max_block = 64 KiB` とする。`RegionConfig` は内部で `initial_block` を `256` 以上に、`max_block` を `initial_block` 以上に正規化する。
- `initial_block` / `max_block` を小さくすると、固定容量アリーナ（`ArenaAllocator` 等）から切り出すチャンクの丸め無駄を抑えられる（一方、`memory_bytes` は領域内の論理使用バイト数を数えるため、ブロックを小さくしても必要な最小 `memory_bytes` は減らない。参考：最小限の評価に必要な `memory_bytes` はリテラルで約 1 KiB、単純な算術式で約 1.6 KiB、関数定義を含む式で約 3.5 KiB 程度。`Allocator` からの実確保総量はブロック単位の切り上げにより `memory_bytes` を超えうるため、アリーナ容量は別に見積もる）。

### 1.4 資源上限 `Limits` とエラー `Error`

> `Limits`・`ErrorKind`・`Span` は常に利用可能。`StackFrame`・`Error` は `std-value` フィーチャ有効時のみ提供される（`--no-default-features` では 1.2.1 の `ErrorRef` を使用する）。

```rust
pub struct Limits {
    pub steps: usize,        // 既定: 100_000
    pub depth: usize,        // 既定: 200（WorkArena 上の非再帰フレームで管理）
    pub memory_bytes: usize, // 既定: 16 * 1024 * 1024（3 領域合計の論理使用バイト数）
}

pub enum ErrorKind {
    Lex,
    Parse,
    Type,
    Runtime,
    TooManySteps,
    TooDeep,
    TooMuchMemory,
}

pub struct StackFrame {
    pub module: Option<String>,
    pub name: Option<String>,
    pub caller_module: Option<String>,
    pub at: Span,
    pub tail_calls: usize,
    pub tail_target_module: Option<String>,
    pub tail_target_name: Option<String>,
    pub repeat_count: usize,
    pub from_host: bool,
    pub via_builtin: Option<String>,
}

pub struct Error {
    pub kind: ErrorKind,
    pub at: Span, // 1始まりの line, col（col は UTF-8 バイト位置）
    pub message: String,
}

impl Error {
    pub fn rule(&self) -> &'static str; // 角括弧を含まない規則名
    pub fn module(&self) -> Option<&str>;
    pub fn backtrace(&self) -> &[StackFrame];
    pub fn omitted_frames(&self) -> usize;
    pub fn format_backtrace(&self) -> String;
}
```

`Error::new(rule, kind, at, message)` と `Error::lex/parse/runtime(rule, at, message)` はすべて `&'static str` の規則名を必須とする（空文字は panic）。表示の先頭行は `L:C: message [rule.id]`、モジュール内では `module L:C: message [rule.id]` となり、その後に呼び出し履歴が続く。`message` と `format_backtrace()` の内容には規則名を追加しない。

名前解決に失敗したときは、候補名やフィールド・公開シンボル一覧を `Error.message` に含める。表示順・件数上限・候補の選択規則は [言語仕様の `[eval.hints]`](spec.md) に従う。C FFI の `RinError.message` / `text` にも含まれる。

`depth` は、ユーザー定義関数の呼び出しの入れ子の深さを制限する。組み込み関数からの呼び出しも含めるが、末尾呼び出し最適化が適用された呼び出しは深さを増やさない。VM と高階組み込みコールバックは `WorkArena` 上の明示フレームスタックで非再帰に実行するため、ネイティブスタック容量に依存せず、`depth` 上限と利用可能なメモリ（`memory_bytes`）の範囲内で任意の深さまで実行できる（256 KiB スレッドスタック上で深さ 100,000 の非末尾再帰を検証済み）。

### 1.5 深い値の処理（`std-value` 側の `Value` とゼロヒープ経路）

`std-value` フィーチャ有効時のホスト側所有コンテナおよび識別子値は `Value::ident`・`Value::array`・`tuple`・`structure`・`variant`・`some`・`ok`・`err` で構築する（`Value::Ident` は有効な識別子名のみ取り込める）。`Array` / `Tuple` の要素は `Rc<ValueItems>`、`Struct` のフィールドは `Rc<ValueFields>`、option / result / variant の子は `Rc<ValueChild>` が所有する。これらの型は `rin::value` で公開し、元の `Vec<Value>`・`BTreeMap<String, Value>`・`Value` への `Deref` / `DerefMut` と `From` を提供する。

解放処理はこれらの所有データと `ListNode` に実装している。`Value` 自身は独自の `Drop` を持たないため、パターンマッチで所有データを取り出せる。共有参照の解放では参照カウントを減らすだけで、最後の所有者になったときだけ子を処理する。

ソースの構文入れ子上限とは別に、繰り返し処理やホストから渡す値は深く入れ子になり得る。次の処理は明示的な作業スタックまたは侵入型キューを使い、値の深さに比例してネイティブスタックを消費しない。

- ホストと内部プールの値変換（`std-value` の import / export）。`bind`・評価結果・関数の引数と戻り値を含む。
- `Value` の `Display`・`Debug`・`PartialEq`・`approx_bytes`、`ListNode` の `Debug`・等値比較、および Rin 内の `==` / `!=` と `ValueRef` の `Display`。
- `Schema::check`（`std-value` 有効時の C FFI `rin_check` / `rin_check_str` を含む）。値を消費せず同じ参照を巡回するスキーマは検証エラーとなる。
- `Value` / `ListNode` の解放および内部プールオブジェクトの解放（追加メモリ確保なし・非再帰の反復解放）。共有された子は残し、最後の強参照を解放する際に子を分解する。Weak 参照が残る場合も扱う。

import がメモリ上限・アロケータの確保失敗・取り込めない値で失敗した場合、生成途中の値を回収する。既存の束縛は保持し、同じ `Builder` または `Machine` を引き続き使える。一方、ゼロヒープ経路（Rust の `Val` / `ValueRef` および C FFI の `RinValue`）は `Value` を経由せず、渡された `Allocator` 上のプール値を直接操作する。

`std-value` の所有型 `Value` に関する変換・解放・比較・書式化はホストのシステムヒープを使用し、`memory_bytes` の計数対象外である（変換・解放は深さ 32、比較・書式化は深さ 16 まで直接処理し、それ以降は反復処理で進める）。一方、ゼロヒープ経路（`Val` / `ValueRef` / `ErrorRef` / C FFI）の確保はすべて渡された `Allocator` 上で行い、`memory_bytes` の管理下にある。

`Debug` は入れ子を省略せず、`{:#?}` は浅い部分を字下げして出力する。書式化の深さはリストの各セル（tail 方向）も 1 段と数えるため、平らな長いリストも途中で切り替わる。書式化の深さ上限に達した部分木は、スタック消費を抑えるためコンパクトな形式で出力する。

### 1.6 ソース付きエラー表示（`std-value` フィーチャ）

```rust
impl Error {
    pub fn render(&self, source: &str) -> String;
    pub fn render_with<'s>(
        &self,
        source: impl FnMut(Option<&str>) -> Option<&'s str>,
    ) -> String;
    pub fn render_to(
        &self, out: &mut (impl std::fmt::Write + ?Sized), source: &str,
    ) -> std::fmt::Result;
    pub fn render_with_to<'s>(
        &self,
        out: &mut (impl std::fmt::Write + ?Sized),
        source: impl FnMut(Option<&str>) -> Option<&'s str>,
    ) -> std::fmt::Result;
}
```

`render` にエラーが発生したソースコード全体を渡すと、該当行に `^` キャレットを付けて、エラーメッセージとともに表示する文字列を返す。呼び出し履歴にも同じソースコード内の行を表示する。別のモジュールからの呼び出しについては、履歴だけを表示する。

複数のモジュールのソースコードを表示するには、`render_with` を使う。引数には、モジュール名を受け取ってソースコード全体を返すコールバックを渡す。エラーの発生箇所には、そのモジュールのソースコードを使う。呼び出し履歴には、関数を呼び出した側のモジュールのソースコードを使う。

- コールバックが `Some(name)` を受け取ったら、名前が `name` のモジュールのソースコードを `Some(source)` で返す。
- `None` を受け取ったら、モジュールに属さないコード（`eval` に渡したコードなど）を `Some(source)` で返す。
- 対応するソースコードを用意できなければ、`None` を返す。その箇所はエラーメッセージや呼び出し履歴だけを表示する。

コールバックは表示する位置ごとに 1 回呼ばれるため、同じモジュール名を複数回受け取ることがある。

渡すソースコードは、エラーが発生したときに実行していたものと一致させる。たとえば、別の環境や更新後のソースコードを渡すと行番号が合わないことがある。実行時のソースコードを特定できなければ、コールバックから `None` を返す。

`render_to` / `render_with_to` は同じ表示を `fmt::Write` へ追記し、書き込みが失敗した時点で `fmt::Error` を返す。末尾の改行は付けない。表示幅・無効な位置の扱いは言語仕様の `[eval.snippet]` に従う。

```rust
let source = "(/ 1 0)";
let error = rin::eval(source).unwrap_err();
println!("{}", error.render(source));
```

```text
1:1: division by zero [op.arith]
  |
1 | (/ 1 0)
  | ^
```

ソース付き表示はこの API を明示的に呼んだときだけ生成する。`Error.message`、通常の `Display`、C FFI の `RinError.message` / `text` はソースを含まない。

---

### テスト実行API（`std-value` フィーチャ）

```rust
pub fn rin::test::run(
    sources: &[(&str, &str)],
    modules: &[(&str, &str)],
    limits: Limits,
) -> rin::test::TestReport;

pub fn rin::test::run_filtered(
    sources: &[(&str, &str)],
    modules: &[(&str, &str)],
    limits: Limits,
    filter: Option<&str>,
) -> rin::test::TestReport;
```

`sources` はファイル名とソース、`modules` はモジュール名とソースの組を渡す。`filter` はテスト名の部分一致で、`None` は全件を実行する。テスト実行時の数学関数の FMA 設定は常に `MathFma::Strict` とする。

`TestReport.tests` は各テストの `source`、`name`、`at`、`status`、`failures`、`error` を保持する。`status` は `TestStatus::{Passed, Failed, Error}`。`CheckFailure` は `at`、`actual`、`expected` と、`check_eq` なら真となる `equality` を持つ。値は所有された `Value` で、実行環境を破棄した後も参照できる。

構文やモジュール依存のエラーは `TestReport.errors` の `SourceError { source, error }` に入る。`is_success()` は全テストが成功し、準備段階のエラーもない場合に真を返す。テストが0件でエラーがない場合も真となる。実行順と上限の適用範囲は[テストツール仕様](testing.md)を参照。

### ソース整形API（`std-value` フィーチャ）

```rust
pub fn rin::fmt::format(source: &str) -> Result<String, Error>;
```

ソースを共通の書式に整形して返す。コメント・リテラル・構文を保持し、整形前後のASTの同一性を検査する。入力の構文エラー、または整形による構文の変化は `Error` を返す。書式とCLIは[フォーマッタ仕様](fmt.md)を参照。

## 2. C FFI API

ヘッダ：[rin.h](../rin/include/rin.h)

### 2.1 型定義

- `RinTag`：`RIN_TAG_VOID`, `RIN_TAG_INT`, `RIN_TAG_FLOAT`, `RIN_TAG_BOOL`, `RIN_TAG_CHAR`, `RIN_TAG_TEXT`, `RIN_TAG_LIST`, `RIN_TAG_ARRAY`, `RIN_TAG_TUPLE`, `RIN_TAG_STRUCT`, `RIN_TAG_VARIANT`, `RIN_TAG_NONE`, `RIN_TAG_SOME`, `RIN_TAG_OK`, `RIN_TAG_ERR`, `RIN_TAG_FN`, `RIN_TAG_IDENT`
- `RinLimits`：`{ size_t steps; size_t depth; size_t memory_bytes; }`
- `RinRegionConfig`：`{ size_t initial_block; size_t max_block; }`（内部で `initial_block` は `256` 以上に、`max_block` は `initial_block` 以上に正規化される）
- `RinMemoryConfig`：`{ RinRegionConfig perm; RinRegionConfig obj; RinRegionConfig work; }`
- `RinMathFma`：`RIN_MATH_FMA_STRICT = 0`, `RIN_MATH_FMA_FUSED = 1`
- `RinAllocator`：`{ void *ctx; uint8_t *(*alloc)(void *ctx, size_t size, size_t align); void (*dealloc)(void *ctx, uint8_t *ptr, size_t size, size_t align); }`
  - `alloc` が `NULL` の場合は、`dealloc` の有無によらず標準ヒープ（`HeapAllocator`）で確保・解放する（`dealloc` は呼ばれない）。
  - `alloc` が非 `NULL` の場合、成功時は `align` に整列した少なくとも `size` バイトの読み書き可能な有効領域を返し、同時に生存する他の確保領域と重複させず、対応する `dealloc` または `rin_free` まで有効に保つこと。
  - `alloc` が非 `NULL` かつ `dealloc` が `NULL` の場合は個別解放を省略する（アリーナ用途）。その場合も、生存中の領域を早期に再利用してはならない。
  - `dealloc` には確保時と同一の `ptr`・`size`・`align` が渡される。
  - `size == 0` の要求では C の `alloc` / `dealloc` のいずれも呼び出さない。
  - `ctx` と `alloc` / `dealloc` が指す関数は、`rin_new`（または `rin_new_with_config`）の呼び出しから `rin_free` が戻るまで有効に保つこと（`rin_free` は `Rin` 本体の解放にも `dealloc` を使用する）。
- `Rin` / `RinValue`：不透明ハンドル型。`Rin` 本体・`RinValue` ハンドル・`RinError` バッファはすべて `RinAllocator` 上に直接確保され、カスタムアロケータ使用時はシステムヒープを一切使わない。
- `RinType`：`{ uint32_t id; }`（`rin_resolve_type` で事前解決した型ハンドル）
- `RinBinding`：`{ const char *name; const char *type_expr; const RinValue *value; }`
- `RinTypedBinding`：`{ const char *name; RinType ty; const RinValue *value; }`
- `RinEvalResult`：`{ bool ok; const RinValue *value; }`（`value` は `Rin` が所有する借用ポインタ。同じ `Rin` に対する次の評価・呼び出し・解放まで有効。**`rin_drop` へ渡してはならない**。有効期間中であれば、そのまま後続の `rin_call` / `rin_bind` 等への入力引数として渡せる）

### 2.2 `Rin` 操作

| 関数 | 説明 |
|---|---|
| `RinLimits rin_limits_default(void)` | 既定の `RinLimits` を返す |
| `RinMemoryConfig rin_memory_config_default(void)` | 既定の `RinMemoryConfig`（各領域 `initial_block = 4 KiB`、`max_block = 64 KiB`）を返す |
| `Rin *rin_new(RinLimits limits, RinAllocator alloc)` | 読み込み段階の `Rin` を `alloc` 上に生成する（確保失敗時は `NULL`） |
| `Rin *rin_new_with_config(RinLimits limits, RinMemoryConfig memory, RinAllocator alloc)` | 3 領域のブロック設定 `memory`（`initial_block >= 256`、`max_block >= initial_block` へ正規化）を指定して `Rin` を `alloc` 上に生成する（確保失敗時は `NULL`） |
| `void rin_free(Rin *rin)` | `Rin` と保持中の全ハンドル・プールを `alloc` へ解放する（`NULL` 可） |
| `bool rin_freeze(Rin *rin)` | 大域環境と読み込み済みモジュールを凍結し、実行の段階へ移行する（`rin == NULL` のときのみ `false`、すでに凍結済みなら `true`） |
| `bool rin_set_math_fma(Rin *rin, RinMathFma mode)` | 組み込み数学関数の FMA 設定を変更する（不正値・NULL は `false`、非 NULL なら `RIN_ERR_HOST` を記録） |
| `RinMathFma rin_math_fma(const Rin *rin)` | 現在の FMA 設定を返す（`NULL` は `RIN_MATH_FMA_STRICT`） |
| `bool rin_resolve_type(Rin *rin, const char *type_expr, RinType *out)` | 読み込み段階で型式を事前解決して `*out` へ書き込む（`fn` 型を含む場合は `type.boundary`、凍結後は `env.frozen`） |
| `bool rin_bind(Rin *rin, const char *name, const char *type_expr, const RinValue *val)` | 値を型式 `type_expr` で検証したうえで大域変数として束縛する（所有権は移動しない。凍結後は `RIN_ERR_RUNTIME`・規則 `env.frozen`・位置 `0:0`、型不一致は `RIN_ERR_RUNTIME`・規則 `type.boundary`・位置 `0:0`、既存の大域束縛・組み込み名・読み込み済みモジュール名との衝突は `RIN_ERR_RUNTIME`・規則 `expr.def`・位置 `0:0` を記録して `false`） |
| `bool rin_bind_typed(Rin *rin, const char *name, RinType ty, const RinValue *val)` | 事前解決済み `RinType` を使って値を大域変数として束縛する |
| `RinEvalResult rin_build(Rin *rin, const char *source)` | 読み込み段階でソースを評価し大域定義を構築する（凍結後は `RIN_ERR_RUNTIME`・規則 `env.frozen`・位置 `0:0` を記録して失敗。評価が途中で失敗した場合は追加した大域束縛・`import` を取り消す） |
| `RinEvalResult rin_eval(Rin *rin, const char *source)` | 凍結後の環境でソースを評価し結果を借用ポインタで返す（`[env.frozen]`・`[env.modules]` を適用。凍結前は `RIN_ERR_RUNTIME`・規則 `env.phase`・位置 `0:0` を記録して失敗） |
| `RinEvalResult rin_eval_with(Rin *rin, const char *source, const RinBinding *bindings, size_t binding_count)` | 凍結後の環境で、この 1 回の評価にだけ有効な初期束縛 `bindings` を与えてソースを評価する（評価するソースの最外側のレキシカル束縛として導入され、そのソースおよびその中で生成されたクロージャからのみ参照でき、凍結済み関数からは参照されない。前提：`bindings` は `binding_count` 個の有効な `RinBinding` を指し、各要素の `name` は NUL 終端 UTF-8、`value` は同じ `Rin` で生成した有効な `RinValue *` で呼び出し中のみ有効であればよい。所有権は移動しない。同じ名前が重複した場合は後ろの束縛が有効になる。`binding_count == 0` なら `bindings == NULL` 可。`binding_count > 0` で `bindings == NULL`、要素の `name` / `value` が `NULL`、または `name` が不正 UTF-8 のときは `RIN_ERR_HOST`・規則 `host.argument`。凍結前は `RIN_ERR_RUNTIME`・規則 `env.phase`・位置 `0:0` を記録して失敗） |
| `RinEvalResult rin_eval_with_typed(Rin *rin, const char *source, const RinTypedBinding *bindings, size_t binding_count)` | 事前解決済み `RinType` を使って `rin_eval_with` と同様に評価する（型式文字列の再パースを省略） |
| `RinEvalResult rin_load(Rin *rin, const char *mod_name, const char *source)` | ホスト提供のソースを名前付きモジュールとして読み込む（凍結後は `RIN_ERR_RUNTIME`・規則 `env.frozen`・位置 `0:0`、読み込み済みモジュール名や既存の大域束縛・組み込み名との衝突は `RIN_ERR_RUNTIME`・規則 `expr.module`・位置 `0:0` を記録して失敗。失敗した読み込みは登録されず再試行可能） |
| `RinEvalResult rin_call(Rin *rin, const char *fn_name, const RinValue *const *args, size_t argc)` | 定義済み関数を文字列再パースなしで直接呼び出す（凍結前・凍結後のどちらでも呼び出せる） |
| `RinEvalResult rin_call_in(Rin *rin, const char *mod_name, const char *fn_name, const RinValue *const *args, size_t argc)` | 読み込んだモジュール内の公開関数を直接呼び出す（凍結前・凍結後のどちらでも呼び出せる） |
| `bool rin_fn_arity(const Rin *rin, const char *fn_name, size_t *out_arity)` | 大域関数 `defn` の仮引数個数を `*out_arity` へ書き込む |
| `bool rin_fn_arity_in(const Rin *rin, const char *mod_name, const char *fn_name, size_t *out_arity)` | モジュール公開関数 `defn` の仮引数個数を `*out_arity` へ書き込む |
| `size_t rin_fn_sig_format(const Rin *rin, const char *fn_name, char *out_buf, size_t out_cap)` | 大域関数 `defn` のシグネチャを `out_buf`（NUL 終端）へ書き出し、必要なバイト数（NUL 除く）を返す（未定義なら `0`） |
| `size_t rin_fn_sig_format_in(const Rin *rin, const char *mod_name, const char *fn_name, char *out_buf, size_t out_cap)` | モジュール公開関数 `defn` のシグネチャを `out_buf`（NUL 終端）へ書き出し、必要なバイト数（NUL 除く）を返す（未定義なら `0`） |
| `const RinError *rin_last_error(const Rin *rin)` | 直近の対象操作のエラーを借用する（エラーなし・NULL は NULL） |

#### エラー情報の借用

`RinError` は次の情報を持つ。ホストは構造体・文字列のいずれも解放しない。

```c
typedef enum RinErrorKind {
    RIN_ERR_LEX = 0, RIN_ERR_PARSE = 1, RIN_ERR_RUNTIME = 2,
    RIN_ERR_TOO_MANY_STEPS = 3, RIN_ERR_TOO_DEEP = 4,
    RIN_ERR_TOO_MUCH_MEMORY = 5, RIN_ERR_HOST = 6, RIN_ERR_TYPE = 7
} RinErrorKind;

typedef struct RinError {
    RinErrorKind kind;
    const char *message;
    const char *module;
    const char *rule;
    size_t line;
    size_t col;
    const char *backtrace;
    const char *text;
} RinError;
```

- `message` は見出しの本文、`text` は Rust の `Error::to_string()` 相当の見出しと履歴全体であり、必ず非 NULL の UTF-8・NUL 終端文字列である。内部 NUL は可視の `\0` に置き換える。ほかの文字列にも同じ変換を適用する。
- **インライン文字列バッファ上限（NUL 終端込み）**：システムヒープを使わずに保持するため、C FFI 側の各文字列には固定長上限がある：`rule` は 64 バイト、`message` は 1024 バイト、`module` は 128 バイト、`backtrace` は 2048 バイト、`text` は 3200 バイト（各最大文字列長は上限 − 1 バイト）。上限を超える場合は UTF-8 文字境界で切り詰められる。履歴が長い場合は `backtrace`（最大 2047 バイト）が `text`（最大 3199 バイト）より手前で切れることがある。また、コアの履歴文字列プール（2048 バイト）をあふれたフレームの名前は途中切り詰めまたは空文字列となる。
- `module` は発生モジュール（モジュール外なら NULL）、`backtrace` は整形済み履歴（なければ NULL）、`rule` は根拠となる規則名（角括弧なし、非 NULL）とする。
- `kind` の先頭 6 種は Rust の `ErrorKind` に対応する。`RIN_ERR_HOST` は FFI の引数エラーである。今後の種別は末尾に追加する。
- `line` / `col` は 1 始まりで、ソース位置がなければ 0 とする。
- 状態変更・評価操作は `rin_build` / `rin_eval` / `rin_eval_with` / `rin_load` / `rin_call` / `rin_call_in` / `rin_bind` / `rin_freeze` / `rin_set_math_fma` の 9 関数であり、冒頭で前回のエラーをクリアし、失敗時に新しいエラーを記録する。また、値構築関数（`rin_int` / `rin_text` / `rin_cons` / `rin_struct` 等）・`rin_struct_set` の失敗（OOM・NULL 引数・不正 UTF-8 等）、および所有ハンドルを返す読み取り 5 関数（`rin_car` / `rin_cdr` / `rin_field` / `rin_unwrap` / `rin_tuple_get`）のハンドル確保失敗時にも `NULL` / `false` を返して `rin_last_error` を更新する。返却ポインタと文字列は、同じ `Rin` への次のこれら操作、または `rin_free` まで有効である。`rin_last_error` の再取得では無効化しない。同じ `Rin` の操作・解放と並行して参照してはならない。
- 有効な `Rin` への状態変更・評価操作が失敗すると必ずエラーを記録し、成功すると `rin_last_error` は NULL を返す。`rin_bind` の内部処理の失敗も記録する。
- NULL 引数・不正 UTF-8 は `RIN_ERR_HOST` とし、`module` / `backtrace` は NULL、`line` / `col` は 0、`rule` は `host.argument` とする。`message` は引数エラーの文言、`text` はその末尾に ` [host.argument]` を付けた文字列とする。`Rin` 自体が NULL の場合は記録先がないため記録せず、操作は失敗する。`rin_last_error(NULL)` は NULL を返す。
- `RinEvalResult.value` は評価・関数呼び出し成功時には `Rin` が所有する借用ポインタ（`const RinValue *`）であり、同じ `Rin` への次の評価・呼び出し・解放まで有効である（**ホスト側で `rin_drop` に渡してはならない**。有効期間中であれば、そのまま後続の `rin_call` / `rin_bind` 等の入力値として渡せる）。`rin_load` 成功時は `{ true, NULL }`、失敗時は `{ false, NULL }` となる。

凍結済みの有効な `r` に対する利用例：

```c
RinEvalResult result = rin_eval(r, source);
if (!result.ok) {
    const RinError *error = rin_last_error(r);
    fprintf(stderr, "%s\n", error->text);
}
```

### 2.3 値の生成と解放

生成関数は `Rin *rin` を第 1 引数に取り、`rin` のアロケータ上で値ハンドルを確保して返す。使い終わったら同じ `rin` に対して `rin_drop(rin, val)` で返却する。

- **失敗時の挙動**：`rin == NULL`、必須引数の `NULL`、不正 UTF-8 文字列、`rin_struct_set` の第 2 引数がその `Rin` の生存中ハンドルでない場合、メモリ上限（`memory_bytes`）超過、またはアロケータの確保失敗時は `NULL`（`rin_struct_set` は `false`）を返し、`rin != NULL` であれば `rin_last_error(rin)` にエラー（`RIN_ERR_HOST` または `RIN_ERR_TOO_MUCH_MEMORY`）を記録する。
- **所有権**：`rin_cons` / `rin_some` / `rin_struct_set` / `rin_variant` / `rin_bind` / `rin_call` / `rin_eval_with` は渡された `RinValue *` を内部で retain するため、呼び出し元の所有権は移動しない。呼び出し元は自身が構築した `RinValue *` を `rin_drop` で解放する。`RinEvalResult.value` の借用ポインタは `rin_drop` や `rin_struct_set` の第 2 引数へ渡してはならない。
- **ハンドルの取り違えと解放済みハンドル**：次はホスト側の誤用であり、してはならない（`[eval.sandbox]` の対象ではなく、`rin_struct_set` の場合を除き `rin_last_error` にも記録されない）。
  - 同じハンドルを 2 回 `rin_drop` した場合は内部の生存フラグ（`alive`）により 2 回目は無視される。ただしスロットが新しいハンドルへ再利用された後に古いポインタを渡すと、`rin_drop` では新しいハンドルを解放してしまい、`rin_struct_set` の第 2 引数では再利用先のハンドルが持つ構造体を書き換えてしまう。
  - `rin_drop` 済みのハンドル（未再利用のもの）および `RinEvalResult.value` を `rin_struct_set` の第 2 引数に渡した場合は `RIN_ERR_HOST` を記録して `false` を返す。`rin_drop` 済みのハンドルを読み取り・入力に渡した場合は、`Void` またはスロット再利用後の別の値を読むことがある。
  - 別の生存中 `Rin` で生成した `RinValue *` を `rin_drop` に渡した場合は O(1) の所有検査により無視され、`rin_struct_set` の第 2 引数に渡した場合は `RIN_ERR_HOST` を記録して `false` を返す。読み取り・入力に渡した場合は、別の値を読むか、内部の `ObjId` 境界検査でパニックする（Rust の `extern "C"` 境界を跨ぐパニックはプロセスの中断（abort）となる。メモリの範囲外には触れない）。なお、`rin_free` した `Rin` のハンドルは無効なポインタであり、どの関数にも渡してはならない。

| 関数 | 説明 |
|---|---|
| `RinValue *rin_int(Rin *rin, int64_t n)` | `i64` 値を生成する |
| `RinValue *rin_float(Rin *rin, double x)` | `f64` 値を生成する |
| `RinValue *rin_bool(Rin *rin, bool b)` | `bool` 値を生成する |
| `RinValue *rin_text(Rin *rin, const char *s)` | UTF-8 の `text` 値を生成する |
| `RinValue *rin_ident(Rin *rin, const char *s)` | UTF-8 識別子値（`ident`）を生成する（無効な識別子名は `RIN_ERR_HOST` を記録して `NULL` を返す） |
| `RinValue *rin_i64s(Rin *rin, const int64_t *ptr, size_t len)` | `i64` の `array` を生成する |
| `RinValue *rin_f64s(Rin *rin, const double *ptr, size_t len)` | `f64` の `array` を生成する |
| `RinValue *rin_nil(Rin *rin)` | 空リスト `[]` を生成する |
| `RinValue *rin_cons(Rin *rin, const RinValue *head, const RinValue *tail)` | `tail` の先頭に `head` を追加した新リストを返す（O(1)） |
| `RinValue *rin_none(Rin *rin)` | `none` 値を生成する |
| `RinValue *rin_some(Rin *rin, const RinValue *val)` | `some(val)` 値を生成する |
| `RinValue *rin_struct(Rin *rin, const char *name)` | 空の `Struct` 値を生成する |
| `bool rin_struct_set(Rin *rin, RinValue *s, const char *field, const RinValue *val)` | `Struct` のフィールドを設定する |
| `RinValue *rin_variant(Rin *rin, const char *type_name, const char *variant, const RinValue *data)` | `Variant` 値を生成する（`data` は `NULL` 可） |
| `void rin_drop(Rin *rin, RinValue *val)` | `RinValue *` ハンドルを解放する（ハンドルの返却は O(1)。最後の参照だった場合は、値の解放の費用（`[value.release-cost]`）が加わる。`NULL` 可） |

### 2.4 値の読み取り

> 所有 `RinValue *` を返す 5 関数（`rin_car` / `rin_cdr` / `rin_field` / `rin_unwrap` / `rin_tuple_get`）は、ハンドル領域の確保に失敗した場合にも `NULL` を返して `rin_last_error(rin)` を更新する。

| 関数 | 説明 |
|---|---|
| `RinTag rin_tag(const Rin *rin, const RinValue *val)` | 値の型タグを返す |
| `bool rin_as_int(const Rin *rin, const RinValue *val, int64_t *out)` | `i64` なら `*out` に書き込み `true` を返す |
| `bool rin_as_float(const Rin *rin, const RinValue *val, double *out)` | `f64` なら `*out` に書き込み `true` を返す |
| `bool rin_as_bool(const Rin *rin, const RinValue *val, bool *out)` | `bool` なら `*out` に書き込み `true` を返す |
| `const uint8_t *rin_as_text(const Rin *rin, const RinValue *val, size_t *out_len)` | `Text` の UTF-8 バイト列を借用して返す（**NUL 終端ではない**。長さは `*out_len` へ書き込み、空文字列は非 NULL ポインタと長さ 0 を返す。`val` が有効な間のみ有効。非 `Text` または `out_len == NULL` は `NULL` を返し、失敗時は `*out_len` を書き換えない） |
| `const uint8_t *rin_as_ident(const Rin *rin, const RinValue *val, size_t *out_len)` | `Ident` の UTF-8 バイト列を借用して返す（`rin_as_text` と同じく NUL 終端ではない。非 `Ident` または `out_len == NULL` は `NULL` を返し `*out_len` を書き換えない） |
| `size_t rin_len(const Rin *rin, const RinValue *val)` | `List`、`Array`、または `Tuple` の要素数を返す |
| `RinValue *rin_car(Rin *rin, const RinValue *val)` | `List` の先頭要素を所有 `RinValue *` で返す（空・非リストは `NULL`、`rin_drop` で解放） |
| `RinValue *rin_cdr(Rin *rin, const RinValue *val)` | `List` の残余リストを所有 `RinValue *` で返す（O(1)、非リストは `NULL`、`rin_drop` で解放） |
| `RinValue *rin_field(Rin *rin, const RinValue *val, const char *field)` | `Struct` のフィールド `field` を所有 `RinValue *` で返す（欠落・非構造体は `NULL`、`rin_drop` で解放） |
| `const uint8_t *rin_variant_name(const Rin *rin, const RinValue *val, size_t *out_len)` | `Variant` のバリアント名 UTF-8 バイト列を借用して返す（**NUL 終端ではない**。長さは `*out_len` へ書き込み、空文字列は非 NULL ポインタと長さ 0 を返す。`val` が有効な間のみ有効。非 `Variant` または `out_len == NULL` は `NULL` を返し、失敗時は `*out_len` を書き換えない） |
| `RinValue *rin_unwrap(Rin *rin, const RinValue *val)` | `Some` / `Ok` / `Err` / `Variant` ペイロードの中身を所有 `RinValue *` で返す（`rin_drop` で解放） |
| `RinValue *rin_tuple_get(Rin *rin, const RinValue *val, size_t index)` | `Tuple` / `Array` の `index` 番目要素を所有 `RinValue *` で返す（`rin_drop` で解放） |
| `size_t rin_copy_i64s(const Rin *rin, const RinValue *val, int64_t *out_ptr, size_t out_cap)` | `i64` の `array` を `out_ptr` へコピーする（失敗時は `SIZE_MAX`） |
| `size_t rin_copy_f64s(const Rin *rin, const RinValue *val, double *out_ptr, size_t out_cap)` | `f64` の `array` を `out_ptr` へコピーする（失敗時は `SIZE_MAX`） |

### 2.5 スキーマ検証（[スキーマ仕様](schema.md)・`std-value` フィーチャ）

> 以下の 4 関数は Cargo フィーチャ `std-value` が有効なビルドでのみエクスポートされる（`--no-default-features` でビルドした `librin.a` には含まれない）。

| 関数 | 説明 |
|---|---|
| `RinSchema *rin_schema(const char *src)` | スキーマ文字列を解析して `RinSchema *` を返す（構文エラー時は `NULL`） |
| `void rin_schema_free(RinSchema *schema)` | `RinSchema *` を解放する（`NULL` 可） |
| `bool rin_check(const Rin *rin, const RinValue *val, const RinSchema *schema)` | `val` が `schema` に適合すれば `true` を返す |
| `bool rin_check_str(const Rin *rin, const RinValue *val, const char *schema_src)` | スキーマ文字列をその場で解析して `val` を検証する |

## 3. Rust API と C FFI API の使い分けと対応

### 3.1 Suz から見た使い分け

| 観点 | Rust API（`rin` クレート） | C FFI API（`rin.h` + `librin.a`） |
|---|---|---|
| 使う場所 | Suz コンパイラ本体（Rust 製）や CLI ツール | Suz が出力した C23 プログラム（ゲーム本体・ランタイム） |
| 主な用途 | comptime 計算（定数・テーブル生成）、整形・テストツール連携 | ゲームの Mod・組み込みスクリプトのサンドボックス実行 |
| 典型フロー | `Builder::new` → `bind` / `load_module` / `eval` → `freeze()` → `Machine::eval` / `eval_with` / `call`（単発は `rin::eval_with`） | `rin_new`（Suz の `arena` 等を `RinAllocator` で渡す）→ `rin_bind` / `rin_load` / `rin_build` → `rin_freeze` → `rin_eval` / `rin_eval_with` / `rin_call` / `rin_call_in` → `rin_free` |
| 段階分離の守り方 | Rust の型と所有権ムーブ（`Builder` → `Machine`）でコンパイル時に強制 | `Rin *` 1 本のまま `rin_freeze` で不可逆遷移（凍結前 `rin_eval` は `env.phase`、凍結後 `rin_build`/`rin_bind`/`rin_load` は `env.frozen`） |
| エラー・検証 | `Result<Value, Error>` と `Error::render_with` でコンパイラ診断へ統合 | `rin_last_error`（`RinError`）で取得、`rin_check`（`RinSchema`）で返却値を検証 |

### 3.2 環境・評価・呼び出しの対応

| 操作 | C FFI API | Rust API（ゼロヒープ / `std-value`） | 備考 |
|---|---|---|---|
| ハンドル型 | `Rin *`（1 型） | `Builder<'a>` / `Machine<'a>`（2 型） | C は状態遷移、Rust は型遷移 |
| 既定上限・領域設定 | `rin_limits_default()` / `rin_memory_config_default()` | `Limits::default()` / `MemoryConfig::default()` | |
| 生成 | `rin_new` / `rin_new_with_config` | `Builder::new` / `Builder::with_memory_config` | 読み込み段階で開始 |
| 破棄 | `rin_free(rin)` | `Drop` | |
| 凍結 | `rin_freeze(rin)` | `builder.into_machine()` / `builder.freeze()` | 読み込み → 実行段階へ不可逆遷移 |
| FMA 設定 | `rin_set_math_fma(rin, mode)` | `builder.set_math_fma` / `machine.set_math_fma` | 両段階で利用可 |
| FMA 取得 | `rin_math_fma(rin)` | `builder.math_fma` / `machine.math_fma` | 両段階で利用可 |
| 大域束縛 | `rin_bind(rin, name, val)` | `builder.bind_val` / `builder.bind` | C は凍結後 `[env.frozen]` |
| モジュール読込 | `rin_load(rin, mod_name, src)` | `builder.load_module_ref` / `builder.load_module` | C は凍結後 `[env.frozen]` |
| 読み込み段階の評価 | `rin_build(rin, src)` | `builder.eval_ref` / `builder.eval` | C は凍結後 `[env.frozen]` |
| 実行段階の評価 | `rin_eval(rin, src)` | `machine.eval_ref` / `machine.eval` | C は凍結前 `[env.phase]` |
| 一時束縛付き評価 | `rin_eval_with(rin, src, binds, n)` | `machine.eval_with_vals` / `machine.eval_with` | C は凍結前 `[env.phase]` |
| 関数直接呼出 | `rin_call(rin, fn_name, args, argc)` | `machine.call_ref` / `machine.call` | C は凍結前も呼べる |
| モジュール関数呼出 | `rin_call_in(rin, mod, fn_name, args, argc)` | `machine.call_in_ref` / `machine.call_in` | C は凍結前も呼べる |
| エラー取得 | `rin_last_error(rin)` | `Result<_, ErrorRef<'_>>` / `Result<_, Error>` | C は直前エラーを `Rin` 内に保持 |
| 評価結果の解放 | 不要（借用ポインタ、`rin_drop` 禁止） | `ValueRef` は借用、`Value` は `Drop` | |
| 生存バイト数 | （未提供） | `builder.live_bytes` / `machine.live_bytes` | Rust のみ |

### 3.3 値・スキーマ・その他

| 分類 | C FFI API | Rust API |
|---|---|---|
| 値の生成・破棄 | `rin_int` / `rin_float` / `rin_bool` / `rin_text` / `rin_ident` / `rin_i64s` / `rin_f64s` / `rin_nil` / `rin_cons` / `rin_none` / `rin_some` / `rin_struct` + `rin_struct_set` / `rin_variant` / `rin_drop` | ゼロヒープ: `val_*` / `release_val`、`std-value`: `Value` コンストラクタ・`Drop` |
| 値の読み取り | `rin_tag` / `rin_as_*` / `rin_len` / `rin_car` / `rin_cdr` / `rin_field` / `rin_variant_name` / `rin_unwrap` / `rin_tuple_get` / `rin_copy_*` | ゼロヒープ: `value_ref`（`ValueRef` / `ListIter` / `SliceView` / `StructRef` / `VariantRef`）、`std-value`: `Value` パターンマッチ・各メソッド |
| スキーマ検証（`std-value`） | `rin_schema` / `rin_schema_free` / `rin_check` / `rin_check_str` | `Schema::parse` / `Schema::check` |
| Rust のみ（`std-value`） | （なし） | `rin::eval` / `rin::eval_with` / `rin::eval_with_allocator` / `rin::fmt::format` / `rin::test` / `Error::render` |
