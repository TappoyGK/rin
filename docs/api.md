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
- これら 3 つの簡易評価関数は、読み込み段階の環境で 1 回評価し、数学関数の FMA 設定として常に `MathFma::Strict` を使う。凍結した実行環境や `MathFma::Fused` を使う場合は `Builder` と `Machine` を利用する。

### 1.2 読み込み環境 `Builder<'a>` と凍結環境 `Machine<'a>`

大域環境とモジュールの構築（読み込みの段階）は `Builder<'a>` で行い、`freeze` によって実行専用の `Machine<'a>`（実行の段階）へ移行する。

```rust
impl<'a> Builder<'a> {
    pub fn new(limits: Limits, alloc: &'a mut dyn Allocator) -> Self
    pub fn bind(&mut self, name: &str, value: &Value) -> Result<(), Error>
    pub fn load_module(&mut self, name: &str, source: &str) -> Result<(), Error>
    pub fn eval(&mut self, source: &str) -> Result<Value, Error>
    pub fn set_math_fma(&mut self, mode: MathFma)
    pub fn math_fma(&self) -> MathFma
    pub fn live_bytes(&self) -> usize
    pub fn freeze(self) -> Result<Machine<'a>, Error>
}

impl<'a> Machine<'a> {
    pub fn eval(&mut self, source: &str) -> Result<Value, Error>
    pub fn eval_with(&mut self, source: &str, bindings: &[(&str, Value)]) -> Result<Value, Error>
    pub fn call(&mut self, fn_name: &str, args: &[Value]) -> Result<Value, Error>
    pub fn call_in(&mut self, mod_name: &str, fn_name: &str, args: &[Value]) -> Result<Value, Error>
    pub fn set_math_fma(&mut self, mode: MathFma)
    pub fn math_fma(&self) -> MathFma
    pub fn live_bytes(&self) -> usize
}
```

- `Builder::bind`：ホストの `Value` を内部プールに取り込み、大域変数 `name` として束縛する（同名があれば上書きし、旧値を解放する）。
- `Builder::load_module`：名前付きモジュールを読み込んで評価する。
- `Builder::eval`：読み込み段階でソースを評価する（トップレベルの `def`・`defn`・`import` 等を許可する）。
- `Builder::freeze`：大域環境と読み込み済みモジュールを凍結し、実行段階の `Machine` を返す。将来の永続領域確定エラーに備えて `Result` を返す（現行実装では常に `Ok` を返す）。
- 凍結後の `Machine` から読み込みの段階には戻れない。Mod を読み直す場合は、新しい `Builder` を作って読み込み・凍結し、ホスト側で `Machine` を差し替える（ホットリロードは組み込む側が担う）。
- `Machine::eval` / `Machine::eval_with`：凍結した環境でソースを評価する。`def`・`defn`・`import`・`export` を含むソースは評価前に `[env.frozen]` の構文エラーとする。`eval_with` の `bindings`（初期束縛）はその 1 回の評価にだけ有効で、評価するソースとそこから呼ばれるモジュール外の関数から参照でき（モジュール内の関数からは参照されない）、評価終了時に解放する。読み込み済みのモジュールは `import` なしで `mod_name.symbol` として参照できる。名前解決の優先順位は、局所変数・捕捉変数 → 初期束縛 → 凍結された大域束縛 → 読み込み済みモジュール名とする。
- Rust API では `call` / `call_in` は凍結後の `Machine` のみ提供する（C FFI では同一ハンドル上で `rin_call` / `rin_call_in` を凍結前・凍結後のどちらでも呼び出せる）。
- `set_math_fma` / `math_fma`：組み込み数学関数（`sin`・`cos`）の FMA 使用方針（`MathFma::Strict` / `MathFma::Fused`、既定は `Strict`）を設定・取得する。
- `live_bytes`：現在内部プール上で生存しているオブジェクトの概算バイト数を返す。

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
    pub depth: usize,        // 既定: 200（2 MiB のスレッドスタックで検証）
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

`depth` は、ユーザー定義関数の呼び出しの入れ子の深さを制限する。組み込み関数からの呼び出しも含めるが、末尾呼び出し最適化が適用された呼び出しは深さを増やさない。既定値の 200 は Debug / Release ともに 2 MiB のスレッドスタックで検証している。上限を増やす場合や、より小さいスタックのスレッドから実行する場合は、利用する環境でスタック容量を確認する。

### 1.5 深い値の処理

ホスト側のコンテナは `Value::array`・`tuple`・`structure`・`variant`・`some`・`ok`・`err` で構築する。`Array` / `Tuple` の要素は `Rc<ValueItems>`、`Struct` のフィールドは `Rc<ValueFields>`、option / result / variant の子は `Rc<ValueChild>` が所有する。これらの型は `rin::value` で公開し、元の `Vec<Value>`・`BTreeMap<String, Value>`・`Value` への `Deref` / `DerefMut` と `From` を提供する。

解放処理はこれらの所有データと `ListNode` に実装している。`Value` 自身は独自の `Drop` を持たないため、パターンマッチで所有データを取り出せる。共有参照の解放では参照カウントを減らすだけで、最後の所有者になったときだけ子を処理する。

ソースの構文入れ子上限とは別に、繰り返し処理やホストから渡す値は深く入れ子になり得る。次の処理は明示的な作業スタックを使い、値の深さに比例してネイティブスタックを消費しない。

- ホストと内部プールの値変換（import / export）。`bind`・評価結果・関数の引数と戻り値を含む。
- `Value` の `Display`・`Debug`・`PartialEq`・`approx_bytes`、`ListNode` の `Debug`・等値比較、および Rin 内の `==` / `!=`。
- `Schema::check`（C FFI の `rin_check` / `rin_check_str` を含む）。値を消費せず同じ参照を巡回するスキーマは検証エラーとなる。
- `Value` / `ListNode` の解放。共有された子は残し、最後の強参照を解放する際に子を分解する。Weak 参照が残る場合も扱う。

import がメモリ上限・アロケータの確保失敗・取り込めない値で失敗した場合、生成途中の値を回収する。既存の束縛は保持し、同じ `Builder` または `Machine` を引き続き使える。C FFI の値操作は同じ値表現・解放処理を使用する。

変換と解放は深さ 32、比較と書式化は深さ 16 まで直接処理し、それ以降は反復処理だけで進める。反復処理から深さをリセットして再帰へ戻ることはない。作業スタックの小さい部分は固定長領域に置き、必要な場合だけシステムヒープへ拡張する。この一時領域とホスト側の `Value` は `memory_bytes` の計数対象ではない。すべての確保を指定アロケータへ統合する変更は別途行う。評価器の関数呼び出し深度に関する制約は 1.4 の `depth` と同じである。

`Debug` は入れ子を省略せず、`{:#?}` は浅い部分を字下げして出力する。書式化の深さはリストの各セル（tail 方向）も 1 段と数えるため、平らな長いリストも途中で切り替わる。書式化の深さ上限に達した部分木は、スタック消費を抑えるためコンパクトな形式で出力する。

### 1.6 ソース付きエラー表示

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

渡すソースコードは、エラーが発生したときに実行していたものと一致させる。たとえば、エラー発生後にモジュールを読み込み直した場合、更新後のコードでは行番号が合わないことがある。実行時のソースコードを特定できなければ、コールバックから `None` を返す。

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

### テスト実行API

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

### ソース整形API

```rust
pub fn rin::fmt::format(source: &str) -> Result<String, Error>;
```

ソースを共通の書式に整形して返す。コメント・リテラル・構文を保持し、整形前後のASTの同一性を検査する。入力の構文エラー、または整形による構文の変化は `Error` を返す。書式とCLIは[フォーマッタ仕様](fmt.md)を参照。

## 2. C FFI API

ヘッダ：[rin.h](../rin/include/rin.h)

### 2.1 型定義

- `RinTag`：`RIN_TAG_VOID`, `RIN_TAG_INT`, `RIN_TAG_FLOAT`, `RIN_TAG_BOOL`, `RIN_TAG_CHAR`, `RIN_TAG_STR`, `RIN_TAG_LIST`, `RIN_TAG_ARRAY`, `RIN_TAG_TUPLE`, `RIN_TAG_STRUCT`, `RIN_TAG_VARIANT`, `RIN_TAG_NONE`, `RIN_TAG_SOME`, `RIN_TAG_OK`, `RIN_TAG_ERR`, `RIN_TAG_FN`
- `RinLimits`：`{ size_t steps; size_t depth; size_t memory_bytes; }`
- `RinMathFma`：`RIN_MATH_FMA_STRICT = 0`, `RIN_MATH_FMA_FUSED = 1`
- `RinAllocator`：`{ void *ctx; uint8_t *(*alloc)(void *ctx, size_t size, size_t align); void (*dealloc)(void *ctx, uint8_t *ptr, size_t size, size_t align); }`
  - `alloc` が `NULL` の場合は標準ヒープ（`HeapAllocator`）を使用する。
- `Rin` / `RinValue`：不透明ハンドル型。
- `RinBinding`：`{ const char *name; const RinValue *value; }`
- `RinEvalResult`：`{ bool ok; RinValue *value; }`

### 2.2 `Rin` 操作

| 関数 | 説明 |
|---|---|
| `RinLimits rin_limits_default(void)` | 既定の `RinLimits` を返す |
| `Rin *rin_new(RinLimits limits, RinAllocator alloc)` | 読み込み段階の `Rin` を生成する |
| `void rin_free(Rin *rin)` | `Rin` を破棄し内部プールを解放する（`NULL` 可） |
| `bool rin_freeze(Rin *rin)` | 大域環境と読み込み済みモジュールを凍結し、実行の段階へ移行する（`rin == NULL` のときのみ `false`、すでに凍結済みなら `true`） |
| `bool rin_set_math_fma(Rin *rin, RinMathFma mode)` | 組み込み数学関数の FMA 設定を変更する（不正値・NULL は `false`、非 NULL なら `RIN_ERR_HOST` を記録） |
| `RinMathFma rin_math_fma(const Rin *rin)` | 現在の FMA 設定を返す（`NULL` は `RIN_MATH_FMA_STRICT`） |
| `bool rin_bind(Rin *rin, const char *name, const RinValue *val)` | 値を大域変数として束縛する（所有権は移動しない。凍結後は `RIN_ERR_RUNTIME`・規則 `env.frozen`・位置 `0:0` を記録して `false`） |
| `RinEvalResult rin_build(Rin *rin, const char *source)` | 読み込み段階でソースを評価し大域定義を構築する（凍結後は `RIN_ERR_RUNTIME`・規則 `env.frozen`・位置 `0:0` を記録して失敗） |
| `RinEvalResult rin_eval(Rin *rin, const char *source)` | 凍結後の環境でソースを評価し結果を返す（`[env.frozen]`・`[env.modules]` を適用。凍結前は `RIN_ERR_RUNTIME`・規則 `env.phase`・位置 `0:0` を記録して失敗） |
| `RinEvalResult rin_eval_with(Rin *rin, const char *source, const RinBinding *bindings, size_t binding_count)` | 凍結後の環境で、この 1 回の評価にだけ有効な初期束縛 `bindings` を与えてソースを評価する（所有権は移動しない。`binding_count == 0` なら `bindings == NULL` 可。凍結前は `RIN_ERR_RUNTIME`・規則 `env.phase`・位置 `0:0` を記録して失敗） |
| `RinEvalResult rin_load(Rin *rin, const char *mod_name, const char *source)` | ホスト提供のソースを名前付きモジュールとして読み込む（凍結後は `RIN_ERR_RUNTIME`・規則 `env.frozen`・位置 `0:0` を記録して失敗） |
| `RinEvalResult rin_call(Rin *rin, const char *fn_name, const RinValue *const *args, size_t argc)` | 定義済み関数を文字列再パースなしで直接呼び出す（凍結前・凍結後のどちらでも呼び出せる） |
| `RinEvalResult rin_call_in(Rin *rin, const char *mod_name, const char *fn_name, const RinValue *const *args, size_t argc)` | 読み込んだモジュール内の公開関数を直接呼び出す（凍結前・凍結後のどちらでも呼び出せる） |
| `const RinError *rin_last_error(const Rin *rin)` | 直近の対象操作のエラーを借用する（エラーなし・NULL は NULL） |
| `void rin_res_free(RinEvalResult *res)` | 結果の `value` を解放し NULL にする（エラー情報は保持する） |

#### エラー情報の借用

`RinError` は次の情報を持つ。ホストは構造体・文字列のいずれも解放しない。

```c
typedef enum RinErrorKind {
    RIN_ERR_LEX = 0, RIN_ERR_PARSE = 1, RIN_ERR_RUNTIME = 2,
    RIN_ERR_TOO_MANY_STEPS = 3, RIN_ERR_TOO_DEEP = 4,
    RIN_ERR_TOO_MUCH_MEMORY = 5, RIN_ERR_HOST = 6
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
- `module` は発生モジュール（モジュール外なら NULL）、`backtrace` は整形済み履歴（なければ NULL）、`rule` は根拠となる規則名（角括弧なし、非 NULL）とする。
- `kind` の先頭 6 種は Rust の `ErrorKind` に対応する。`RIN_ERR_HOST` は FFI の引数エラーである。今後の種別は末尾に追加する。
- `line` / `col` は 1 始まりで、ソース位置がなければ 0 とする。
- 対象操作は `rin_build` / `rin_eval` / `rin_eval_with` / `rin_load` / `rin_call` / `rin_call_in` / `rin_bind` / `rin_freeze` / `rin_set_math_fma` の 9 関数である。同じ `Rin` への次の対象操作、または `rin_free` まで、返却ポインタと文字列が有効である。`rin_last_error` の再取得や `rin_res_free` では無効化しない。同じ `Rin` の対象操作・解放と並行して参照してはならない。
- 対象操作は引数検査の前に前回の情報をクリアする。有効な machine への操作が失敗すると必ずエラーを記録し、成功すると `rin_last_error` は NULL を返す。`rin_bind` の内部処理の失敗も記録する。
- NULL 引数・不正 UTF-8 は `RIN_ERR_HOST` とし、`module` / `backtrace` は NULL、`line` / `col` は 0、`rule` は `host.argument` とする。`message` は引数エラーの文言、`text` はその末尾に ` [host.argument]` を付けた文字列とする。machine 自体が NULL の場合は記録先がないため記録せず、操作は失敗する。`rin_last_error(NULL)` は NULL を返す。
- `RinEvalResult.value` は評価・関数呼び出し成功時にはホストが所有する値であり、`rin_res_free` で解放する。`rin_load` 成功時は `{ true, NULL }`、失敗時は `{ false, NULL }` となる。

有効な `m` に対する利用例：

```c
RinEvalResult result = rin_eval(m, source);
if (!result.ok) {
    const RinError *error = rin_last_error(m);
    fprintf(stderr, "%s\n", error->text);
}
rin_res_free(&result);
```

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

| 操作 | C FFI API | Rust API | 備考 |
|---|---|---|---|
| ハンドル型 | `Rin *`（1 型） | `Builder<'a>` / `Machine<'a>`（2 型） | C は状態遷移、Rust は型遷移 |
| 既定上限 | `rin_limits_default()` | `Limits::default()` | |
| 生成 | `rin_new(limits, alloc)` | `Builder::new(limits, &mut alloc)` | 読み込み段階で開始 |
| 破棄 | `rin_free(rin)` | `Drop` | |
| 凍結 | `rin_freeze(rin)` | `builder.freeze()` | 読み込み → 実行段階へ不可逆遷移 |
| FMA 設定 | `rin_set_math_fma(rin, mode)` | `builder.set_math_fma` / `machine.set_math_fma` | 両段階で利用可 |
| FMA 取得 | `rin_math_fma(rin)` | `builder.math_fma` / `machine.math_fma` | 両段階で利用可 |
| 大域束縛 | `rin_bind(rin, name, val)` | `builder.bind(name, &val)` | C は凍結後 `[env.frozen]` |
| モジュール読込 | `rin_load(rin, mod_name, src)` | `builder.load_module(name, src)` | C は凍結後 `[env.frozen]` |
| 読み込み段階の評価 | `rin_build(rin, src)` | `builder.eval(src)` | C は凍結後 `[env.frozen]` |
| 実行段階の評価 | `rin_eval(rin, src)` | `machine.eval(src)` | C は凍結前 `[env.phase]` |
| 一時束縛付き評価 | `rin_eval_with(rin, src, binds, n)` | `machine.eval_with(src, bindings)` | C は凍結前 `[env.phase]` |
| 関数直接呼出 | `rin_call(rin, fn_name, args, argc)` | `machine.call(fn_name, args)` | C は凍結前も呼べる |
| モジュール関数呼出 | `rin_call_in(rin, mod, fn_name, args, argc)` | `machine.call_in(mod, fn_name, args)` | C は凍結前も呼べる |
| エラー取得 | `rin_last_error(rin)` | 各メソッドの `Result<_, Error>` | C は直前エラーを `Rin` 内に保持 |
| 評価結果の解放 | `rin_res_free(&res)` | `Drop` | |
| 生存バイト数 | （未提供） | `builder.live_bytes` / `machine.live_bytes` | Rust のみ |

### 3.3 値・スキーマ・その他

| 分類 | C FFI API | Rust API |
|---|---|---|
| 値の生成・破棄 | `rin_int` / `rin_float` / `rin_bool` / `rin_str` / `rin_i64s` / `rin_f64s` / `rin_nil` / `rin_cons` / `rin_none` / `rin_some` / `rin_struct` + `rin_struct_set` / `rin_variant` / `rin_drop` | `Value` コンストラクタ・`Drop` |
| 値の読み取り | `rin_tag` / `rin_as_*` / `rin_len` / `rin_car` / `rin_cdr` / `rin_field` / `rin_variant_name` / `rin_unwrap` / `rin_tuple_get` / `rin_copy_*` | `Value` パターンマッチ・各メソッド |
| スキーマ検証 | `rin_schema` / `rin_schema_free` / `rin_check` / `rin_check_str` | `Schema::parse` / `Schema::check` |
| Rust のみ | （なし） | `rin::eval` / `rin::eval_with` / `rin::eval_with_allocator` / `rin::fmt::format` / `rin::test` / `Error::render` |
