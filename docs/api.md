# Rin API 仕様

Rin をホストプログラム（Rust および C / FFI）から利用するための API を定める。

---

## 1. Rust API

クレート名：`rin`

### 1.1 簡易評価関数

```rust
pub fn eval(source: &str) -> Result<Value, Error>
pub fn eval_with(source: &str, bindings: &[(&str, Value)], limits: Limits) -> Result<Value, Error>
pub fn eval_with_allocator(
    source: &str,
    bindings: &[(&str, Value)],
    limits: Limits,
    alloc: &mut dyn Allocator,
) -> Result<Value, Error>
```

- `eval`：`Limits::default()`・`HeapAllocator` で式列を評価する。
- `eval_with`：初期束縛 `bindings` と資源上限 `limits` を指定して評価する。
- `eval_with_allocator`：呼び出し元が指定した `Allocator` 上で評価する。

### 1.2 常駐環境 `Machine<'a>`

複数回 `eval` を呼び出し、`def` / `defn` による定義を保持する実行環境である。

```rust
impl<'a> Machine<'a> {
    pub fn new(limits: Limits, alloc: &'a mut dyn Allocator) -> Self
    pub fn bind(&mut self, name: &str, value: &Value) -> Result<(), Error>
    pub fn load_module(&mut self, name: &str, source: &str) -> Result<(), Error>
    pub fn eval(&mut self, source: &str) -> Result<Value, Error>
    pub fn call(&mut self, fn_name: &str, args: &[Value]) -> Result<Value, Error>
    pub fn call_in(&mut self, mod_name: &str, fn_name: &str, args: &[Value]) -> Result<Value, Error>
    pub fn live_bytes(&self) -> usize
}
```

- `bind`：ホストの `Value` を内部プールに取り込み、大域変数 `name` として束縛する（同名があれば上書きし、旧値を解放する）。
- `eval`：ソースを評価し、結果を独立した `Value` として返す。評価中に生成された一時オブジェクトは内部の参照カウントとフリーリストによって回収・再利用される。
- `live_bytes`：現在内部プール上で生存しているオブジェクトの概算バイト数を返す。

### 1.3 カスタムアロケータ `Allocator`

```rust
pub trait Allocator {
    fn alloc(&mut self, layout: Layout) -> Option<NonNull<u8>>;
    unsafe fn dealloc(&mut self, ptr: NonNull<u8>, layout: Layout);
}
```

組み込み実装：
- `HeapAllocator`：Rust 標準ヒープを使うデフォルト実装。
- `ArenaAllocator<'a>`：`ArenaAllocator::new(buf: &'a mut [u8])` で渡したバイト列をバンプ確保する実装（`dealloc` は何もしない）。`used()` / `capacity()` で使用量を確認できる。

### 1.4 資源上限 `Limits` とエラー `Error`

```rust
pub struct Limits {
    pub steps: usize,        // 既定: 100_000
    pub depth: usize,        // 既定: 200
    pub memory_bytes: usize, // 既定: 16 * 1024 * 1024
}

pub enum ErrorKind {
    Lex,
    Parse,
    Runtime,
    TooManySteps,
    TooDeep,
    TooMuchMemory,
}

pub struct StackFrame {
    pub module: Option<String>,
    pub name: Option<String>,
    pub at: Span,
    pub tail_calls: usize,
    pub repeat_count: usize,
    pub from_host: bool,
    pub via_builtin: Option<String>,
}

pub struct Error {
    pub kind: ErrorKind,
    pub at: Span, // 1始まりの line, col
    pub message: String,
}

impl Error {
    pub fn backtrace(&self) -> &[StackFrame];
    pub fn omitted_frames(&self) -> usize;
    pub fn format_backtrace(&self) -> String;
}
```

---

## 2. C FFI API

ヘッダ：[rin.h](file:///Users/tappoy/proj/rin/rin/include/rin.h)

### 2.1 型定義

- `RinTag`：`RIN_TAG_VOID`, `RIN_TAG_INT`, `RIN_TAG_FLOAT`, `RIN_TAG_BOOL`, `RIN_TAG_CHAR`, `RIN_TAG_STR`, `RIN_TAG_LIST`, `RIN_TAG_ARRAY`, `RIN_TAG_TUPLE`, `RIN_TAG_STRUCT`, `RIN_TAG_VARIANT`, `RIN_TAG_NONE`, `RIN_TAG_SOME`, `RIN_TAG_OK`, `RIN_TAG_ERR`, `RIN_TAG_FN`
- `RinLimits`：`{ size_t steps; size_t depth; size_t memory_bytes; }`
- `RinAllocator`：`{ void *ctx; uint8_t *(*alloc)(void *ctx, size_t size, size_t align); void (*dealloc)(void *ctx, uint8_t *ptr, size_t size, size_t align); }`
  - `alloc` が `NULL` の場合は標準ヒープ（`HeapAllocator`）を使用する。
- `RinMachine` / `RinValue`：不透明ハンドル型。
- `RinEvalResult`：`{ bool ok; RinValue *value; char *error_message; size_t error_line; size_t error_col; }`

### 2.2 マシン操作

| 関数 | 説明 |
|---|---|
| `RinLimits rin_limits_default(void)` | 既定の `RinLimits` を返す |
| `RinMachine *rin_new(RinLimits limits, RinAllocator alloc)` | マシンを生成する |
| `void rin_free(RinMachine *machine)` | マシンを破棄し内部プールを解放する（`NULL` 可） |
| `bool rin_bind(RinMachine *machine, const char *name, const RinValue *val)` | 値を大域変数として束縛する（所有権は移動しない） |
| `RinEvalResult rin_eval(RinMachine *machine, const char *source)` | ソースを評価し結果を返す |
| `RinEvalResult rin_load(RinMachine *machine, const char *mod_name, const char *source)` | ホスト提供のソースを名前付きモジュールとして読み込む |
| `RinEvalResult rin_call(RinMachine *machine, const char *fn_name, const RinValue *const *args, size_t argc)` | 定義済み関数を文字列再パースなしで直接呼び出す |
| `RinEvalResult rin_call_in(RinMachine *machine, const char *mod_name, const char *fn_name, const RinValue *const *args, size_t argc)` | 読み込んだモジュール内の公開関数を直接呼び出す |
| `const char *rin_last_backtrace(const RinMachine *machine)` | 直近のエラーのコールスタック文字列を返す（なければ `NULL`、次の呼び出しまたは `rin_free` まで有効） |
| `void rin_res_free(RinEvalResult *res)` | `rin_eval` の結果（`value` / `error_message`）を解放する |

### 2.3 値の生成と解放

生成関数が返す `RinValue *` は呼び出し側が所有し、不要になったら `rin_drop` で解放する。

| 関数 | 説明 |
|---|---|
| `RinValue *rin_int(int64_t n)` | `i64` 値を生成する |
| `RinValue *rin_float(double x)` | `f64` 値を生成する |
| `RinValue *rin_bool(bool b)` | `bool` 値を生成する |
| `RinValue *rin_str(const char *s)` | UTF-8 文字列を生成する |
| `RinValue *rin_i64s(const int64_t *ptr, size_t len)` | `i64` の `array` を生成する |
| `RinValue *rin_f64s(const double *ptr, size_t len)` | `f64` の `array` を生成する |
| `RinValue *rin_nil(void)` | 空リスト `[]` を生成する |
| `RinValue *rin_cons(const RinValue *head, const RinValue *tail)` | `tail` の先頭に `head` を追加した新リストを返す（O(1)） |
| `RinValue *rin_none(void)` | `none` 値を生成する |
| `RinValue *rin_some(const RinValue *val)` | `some(val)` 値を生成する |
| `RinValue *rin_struct(const char *name)` | 空の `Struct` 値を生成する |
| `bool rin_struct_set(RinValue *s, const char *field, const RinValue *val)` | `Struct` のフィールドを設定する |
| `RinValue *rin_variant(const char *type_name, const char *variant, const RinValue *data)` | `Variant` 値を生成する（`data` は `NULL` 可） |
| `void rin_drop(RinValue *val)` | `RinValue *` を解放する（`NULL` 可） |

### 2.4 値の読み取り

| 関数 | 説明 |
|---|---|
| `RinTag rin_tag(const RinValue *val)` | 値の型タグを返す |
| `bool rin_as_int(const RinValue *val, int64_t *out)` | `i64` なら `*out` に書き込み `true` を返す |
| `bool rin_as_float(const RinValue *val, double *out)` | `f64` なら `*out` に書き込み `true` を返す |
| `bool rin_as_bool(const RinValue *val, bool *out)` | `bool` なら `*out` に書き込み `true` を返す |
| `const uint8_t *rin_as_str(const RinValue *val, size_t *out_len)` | `Str` の UTF-8 バイト列を借用して返す（`val` が有効な間のみ有効、非文字列は `NULL`） |
| `size_t rin_len(const RinValue *val)` | `List`、`Array`、または `Tuple` の要素数を返す |
| `RinValue *rin_car(const RinValue *val)` | `List` の先頭要素を新しい `RinValue *` で返す（空・非リストは `NULL`） |
| `RinValue *rin_cdr(const RinValue *val)` | `List` の残余リストを新しい `RinValue *` で返す（O(1)、非リストは `NULL`） |
| `RinValue *rin_field(const RinValue *val, const char *field)` | `Struct` のフィールド `field` を新しい `RinValue *` で返す（欠落・非構造体は `NULL`） |
| `const uint8_t *rin_variant_name(const RinValue *val, size_t *out_len)` | `Variant` のバリアント名 UTF-8 バイト列を借用して返す（`val` が有効な間のみ有効） |
| `RinValue *rin_unwrap(const RinValue *val)` | `Some` / `Ok` / `Err` / `Variant` ペイロードの中身を新しい `RinValue *` で返す |
| `RinValue *rin_tuple_get(const RinValue *val, size_t index)` | `Tuple` / `Array` の `index` 番目要素を新しい `RinValue *` で返す |
| `size_t rin_copy_i64s(const RinValue *val, int64_t *out_ptr, size_t out_cap)` | `i64` の `array` を `out_ptr` へコピーする（失敗時は `SIZE_MAX`） |
| `size_t rin_copy_f64s(const RinValue *val, double *out_ptr, size_t out_cap)` | `f64` の `array` を `out_ptr` へコピーする（失敗時は `SIZE_MAX`） |

### 2.5 スキーマ検証（[スキーマ仕様](schema.md)）

| 関数 | 説明 |
|---|---|
| `RinSchema *rin_schema(const char *src)` | スキーマ文字列を解析して `RinSchema *` を返す（構文エラー時は `NULL`） |
| `void rin_schema_free(RinSchema *schema)` | `RinSchema *` を解放する（`NULL` 可） |
| `bool rin_check(const RinValue *val, const RinSchema *schema)` | `val` が `schema` に適合すれば `true` を返す |
| `bool rin_check_str(const RinValue *val, const char *schema_src)` | スキーマ文字列をその場で解析して `val` を検証する |
