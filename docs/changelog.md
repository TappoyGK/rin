# 変更履歴

利用者に関わる変更をバージョンごとに記載し、関連する PR のリンクを付ける。イシューへのリンクは記載しない。

## 未リリース

- **言語・API の破壊的変更（静的型付けへの移行・`defschema` の言語統合・`defn` 型注釈必須化）**: Rin を静的型付け言語へ移行し、言語構文に `defschema`・型注釈（`def` / `defn` / `fn`）・`datum` 型および型パターン（`i64(n)` 等）・モジュール修飾バリアントパターンを導入した。すべての `defn` で仮引数型と戻り値型 `-> Type` を必須とし、実行前の双方向型検査（`ErrorKind::Type` / `RIN_ERR_TYPE`、`match` 網羅性・到達不能性検査、未実行分岐の型検査を含む）を `WorkArena` 上でゼロヒープに実行する。スキーマ DSL から `any` を廃止して `datum` を追加し、ホスト境界 API（`Builder::bind` / `bind_val`、`Machine::eval_with` / `eval_with_vals`、`eval_with` / `eval_with_allocator`、C FFI `rin_bind` / `RinBinding`）に型式引数を追加して `[type.boundary]` の実行時検証を導入した。また、ゼロヒープの関数シグネチャ・型照会 API（`fn_sig` / `fn_sig_in` / `type_of` / `type_of_in`・`FnSigRef` / `TypeRef`、C FFI `rin_fn_arity` / `rin_fn_sig_format` 等）と、型式再パースを省く事前解決型ハンドル API（`TypeHandle`・`Builder::resolve_type` / `bind_typed` / `bind_value_typed`、`Machine::eval_with_typed` / `eval_with_typed_vals`、C FFI `RinType` / `RinTypedBinding` / `rin_resolve_type` / `rin_bind_typed` / `rin_eval_with_typed`）を追加した（[PR #83](https://github.com/TappoyGK/rin/pull/83)）。

- **言語・API の破壊的変更（`eval_with` のレキシカルスコープ化と同名再定義・モジュール再読込の廃止）**: `Machine::eval_with` / `eval_with_vals` / `rin_eval_with` の初期束縛を動的スコープから評価するソース最外側のレキシカル束縛へ変更し、評価するソースとそこで生成されたクロージャからのみ参照できるようにした（凍結済みの既存関数・クロージャからは参照されない）。また、読み込み段階（`Builder` / `rin_build` / `rin_bind` / `rin_load`）における同一環境での同名再定義・組み込み名の上書き・大域束縛と読み込み済みモジュール名の衝突・読み込み済みモジュールの再読込をすべて実行時エラー（`[expr.def]` / `[expr.defn]` / `[expr.module]`）とし、`Builder::eval` / `rin_build` が途中で失敗した際はその評価で追加した大域束縛・`import` を追加確保なしでロールバックするようにした（同一環境での同じモジュールの再 `import` と、失敗した `load_module` の再試行、`let`・仮引数・初期束縛によるシャドーイングは引き続き許可する）（[PR #81](https://github.com/TappoyGK/rin/pull/81)）。

- **構文追加（複数行文字列リテラル `"""..."""`）**: 閉じ `"""` の行頭空白（スペース・タブ）を基準にインデントを自動除去し、通常の `text` リテラルと同じエスケープおよび `${式}` テンプレート埋め込み（式内の複数行記述を含む）に対応する複数行文字列リテラル `"""..."""` を追加した。`rin fmt` は開始 `"""` から閉じ `"""` までの原文と閉じ行の字下げを保持する（[PR #79](https://github.com/TappoyGK/rin/pull/79)）。

- **構文・API の破壊的変更（`text` 改名・テンプレート埋め込み・浮動小数点表示の Suz 共通化）**: 文字列型名を `string` から `text` に改名し（Rust API: `Value::Text` / `Value::text` / `ValueTag::Text` / `val_text` / `as_text`、C FFI: `RIN_TAG_TEXT` / `rin_text` / `rin_as_text`、スキーマ DSL: `text`）、`f64` の表示を Suz 共通の最短往復規則（`1.0`・`-0.0`・`nan`・`inf`・`-inf`・`|x| >= 1e21` または `0 < |x| < 1e-6` の指数表記）へ統一した。`"..."` 内に `${式}` のテンプレート埋め込みと `\$` エスケープを導入し（生の改行を禁止、値の `Display` で `$` を `\$` と表記）、`text` の `len` を O(1) 化するとともにスカラー値単位の添字 `t[i]`・`(get t i)`・`(slice t start end)`・大小比較（`<` / `<=` / `>` / `>=`）・長さ不一致の即終了・同一リテラルの永続領域共有を追加した（[PR #78](https://github.com/TappoyGK/rin/pull/78)）。

- **構文の破壊的変更（文字リテラル `\a` 形式への移行と文字列エスケープの Suz 共通化）**: 文字リテラルを `'a'` から `\` 始まり（`\a`・`\u{H}`・名前つき文字 `\nul` / `\tab` / `\newline` / `\return` / `\space`）へ変更し、`'` を `quote` 専用とした。文字列のエスケープから `\'` を廃止し `\u{H}` を追加して Suz と共通化し、`Value` / `ValueRef` の `Display` 表現も新しい文字リテラル・エスケープ形式へ更新した（[PR #77](https://github.com/TappoyGK/rin/pull/77)）。

- **構文・API の破壊的変更（字句整理・`quote` / `quasiquote`・`ident` 型・`(tuple ...)` の導入）**: リスト `[e1 e2 ...]`、構造体 `Name { f1: e1 f2: e2 }`、`with` のカンマ区切りと旧タプル構文 `(e1, e2)` を廃止し、空白区切りと `(tuple e1 e2 ...)`（2 要素以上）へ移行した。`'` を文字リテラル（`'a'` / `'\n'` / `'\u{H}'` 等、直後に区切り必須）と `quote` に割り当て、`` ` ``（`quasiquote`）・`,`（`unquote`）・`,@`（`unquote_splicing`）を導入し、識別子中および直後の `'` を字句エラーとした。糖衣構文の正規 S 式形式（`(field ...)` / `(index ...)` / `(some ...)` / `(ok ...)` / `(err ...)` / `(variant ...)` / `(struct ...)` / `(with target (f v) ...)`）を通常のソース構文としても受け入れ、`quote` / `quasiquote` 内で糖衣を正規 S 式へ正規化するようにした。不変の識別子値型 `ident`（`==` / `!=` / `<` / `<=` / `>` / `>=` / `sort` / `match` パターン `'foo` / 組み込み `(ident->text i)`・`(text->ident t)` / Rust `Value::Ident`・`ValueTag::Ident`・`val_ident`・`as_ident` / C FFI `RIN_TAG_IDENT`・`rin_ident`・`rin_as_ident` / スキーマ `ident`）を追加し、値の `Display` 表現を Rin ソースとして再入力可能な形（`[1 2 3]`、`P { x: 1 y: 2 }`、`(tuple 1 2)`、`'foo`）へ更新し、`rin fmt` も新構文と `'` / `` ` `` / `,` / `,@` に対応した（[PR #75](https://github.com/TappoyGK/rin/pull/75)）。

- **Rust API の破壊的変更**: `Allocator` トレイトを `unsafe trait` に変更し、確保領域の有効性・整列・非重複・寿命およびサイズ 0 要求の安全性契約を実装側・呼び出し側それぞれについて定義した（独自実装には `unsafe impl Allocator` が必要）。`HeapAllocator` のサイズ 0 確保も要求アラインメントを満たす非 NULL ポインタを返すように統一し、C FFI の `RinAllocator` ではサイズ 0 要求で `alloc` / `dealloc` を呼ばず、`alloc == NULL` のときは `dealloc` の有無によらず標準ヒープで解放するようにした（[PR #73](https://github.com/TappoyGK/rin/pull/73)）。

- **破壊的変更（メモリ管理・ホスト境界・C FFI の刷新）**: 内部のメモリ管理を 3 領域（永続領域 `PermArena`・オブジェクト領域 `ObjHeap`・作業領域 `WorkArena`）へ移行し、渡された `Allocator`（`ArenaAllocator` を含む）上だけでシステムヒープ確保 0 回で動作するようにした。VM・高階組み込み関数を非再帰化して呼び出し深さ `depth` をネイティブスタックから独立させ、`memory_bytes` は 3 領域（構文解析・コンパイル・作業領域・値）の合計論理使用バイト数へ統一した（同じ `memory_bytes` でも従来より早く `TooMuchMemory` になる場合がある）。エラーストアのインライン化に伴いエラーメッセージは最大 1024 バイト（UTF-8 文字境界切り詰め）・履歴文字列プールは最大 2048 バイトとした。`sort` / `sort_by` の自前安定マージソート化に伴い、比較不能エラー `cannot order A and B` はリスト上で左の要素を `A`・右の要素を `B` として報告する形へ統一した。Rust API にゼロヒープのホスト境界（`Val`・`ValueRef`・`ErrorRef`・`Builder::into_machine`・`Builder::with_memory_config`・`MemoryConfig`・`std-value` フィーチャによるゲート）を追加し、C FFI は `Rin` 本体・`RinValue` ハンドル・`RinError` バッファをすべて `RinAllocator` 上へ配置し、`rin_new_with_config` / `rin_memory_config_default` を追加、値コンストラクタ・読み取り関数に `Rin *` を受け取る形へ変更、値コンストラクタ・`rin_struct_set` の失敗（NULL 引数・不正 UTF-8 の `RIN_ERR_HOST` および確保失敗）と所有ハンドルを返す読み取り 5 関数の確保失敗を `rin_last_error` へ記録するようにし、`RinEvalResult.value` を借用ポインタ（`const RinValue *`）として `rin_res_free` を削除した（[PR #70](https://github.com/TappoyGK/rin/pull/70)）。

- **C API の破壊的変更**: C FFI のハンドル型を `RinMachine` から `Rin` に改名し、読み込み段階専用の `rin_build` と凍結後の一時束縛付き評価 `rin_eval_with`（`RinBinding`）を追加、`rin_eval` を凍結後専用とした（移行手順：`RinMachine` → `Rin`、凍結前の評価は `rin_build`、凍結後は `rin_freeze` のうえで `rin_eval` / `rin_eval_with` を呼ぶ）（[PR #65](https://github.com/TappoyGK/rin/pull/65)）。

- **破壊的変更**: 大域環境の凍結（読み込みの段階と実行の段階の分離）を導入し、`def`・`defn`・`import`・`export` をトップレベル専用（関数本体や式の中は読み込み段階でも `[expr.def]`・`[expr.defn]`・`[expr.module]` の構文エラー）とした。Rust API では `Machine::new`・`bind`・`load_module` を `Builder` へ移し、`Builder::freeze()` で実行専用の `Machine`（`eval`・`eval_with`・`call`・`call_in`）へ移行する形とした。C FFI には `rin_freeze` を追加した。凍結後は `def`・`defn`・`import`・`export` を位置によらず評価前に `[env.frozen]` の構文エラーとし、読み込み済みモジュールを `import` なしで `mod_name.symbol` として参照できる（移行手順：`Machine::new` → `Builder::new`、読み込み完了後に `.freeze()?` で `Machine` を取得する）（[PR #63](https://github.com/TappoyGK/rin/pull/63)）。

- `sin`・`cos` を環境によらずビット一致する自前の実装に置き換え、`[eval.determinism]` の例外を廃止した。`MathFma`（`Strict` / `Fused`、既定は `Strict`）設定と、Suz と共有する手順書（`docs/math-spec.md`）およびテスト値（`docs/math-vectors/sin-cos.tsv`）を追加した（[PR #62](https://github.com/TappoyGK/rin/pull/62)）。

- 組み込み関数の走査・比較・生成と、`with` による構造体の複製を、要素数・文字列長・フィールド数に応じてステップ数へ計上するようにした。同じ `steps` 上限でも、従来より早く `TooManySteps` になる場合がある。関数ごとの計上式に加え、ステップに数えないコンパイル・解放・診断の処理量と制御方法を仕様に定めた（[PR #58](https://github.com/TappoyGK/rin/pull/58)）。

- 同じ入力に対して値・エラー・表示順が一致する条件を、決定性の規則として明文化した。`sin`・`cos`、NaNのビット表現、実際のメモリ確保失敗、ネイティブスタックの保証範囲も定めた（[PR #57](https://github.com/TappoyGK/rin/pull/57)）。

- ソース整形API `rin::fmt::format` と `rin fmt` CLIを追加した。コメント・リテラル・構文の意味を保ちながら書式を揃え、ファイルの書き換え、`--check`、標準入力からの整形に対応する（[PR #54](https://github.com/TappoyGK/rin/pull/54)）。

- ゲームなしでModを検証する `deftest`・`check`・`check_eq`、Rustのテスト実行API、`rin test` CLIを追加した。テストごとの独立した環境、検査失敗の蓄積、ソース付き診断、JSON出力、フィルタと資源上限の指定に対応する。CLIオプションはSuzと同じ形式とし、外部クレートを使わずに実装した（[PR #53](https://github.com/TappoyGK/rin/pull/53)）。
- `unicode-width` への依存を削除した。エラー表示のキャレット位置は、内部の小さな範囲表で求める表示幅から計算する。主な全角文字・結合文字に対応し、絵文字の連結や国旗などの並びは補正しない（[PR #53](https://github.com/TappoyGK/rin/pull/53)）。

- **構文の破壊的変更**：行コメントを `//` から `;` に変更した。文字列・文字リテラル内の `;` はそのまま扱い、`//` は通常の識別子となる（[PR #49](https://github.com/TappoyGK/rin/pull/49)）。

## v0.1.1

- バイトコード VM へ移行し、ローカル変数とキャプチャをスロットで解決するようにした。末尾呼び出し最適化を導入し、最適化される呼び出しは深さ上限に数えない。`and` / `or` を値として扱えるようにした（[PR #4](https://github.com/TappoyGK/rin/pull/4)）。
- 実行時エラーと資源上限エラーに呼び出し履歴を追加した。Rust の `StackFrame` と `Error::backtrace()` で取得でき、エラーの `Display` は履歴を含む複数行の表示となる（[PR #14](https://github.com/TappoyGK/rin/pull/14)）。
- 構文とスキーマ型式の入れ子を最大256段に制限し、超過時は構文エラーを返す。深い構文の解析・コンパイル・解放でネイティブスタックが溢れる問題を修正した（[PR #34](https://github.com/TappoyGK/rin/pull/34)）。
- C FFI のエラー情報を machine が所有する `RinError` と `rin_last_error` に統一した。種別・位置・モジュール・呼び出し履歴を取得でき、返却情報は同じ machine の次の対象操作または解放まで借用できる（[PR #36](https://github.com/TappoyGK/rin/pull/36)）。

- コンパイル時にステップ上限の位置情報を一時配列から最終配列へコピーする処理をなくし、確保とコピーを削減した。ステップ数と `TooManySteps` のエラー位置を保ちながら、短い `eval` の負担を減らした（[PR #45](https://github.com/TappoyGK/rin/pull/45)）。

- 関数のローカル値をキャプチャしたクロージャを末尾の位置で呼ぶと、値が先に解放されて panic する問題を修正した。呼び出し先の実行中はキャプチャした値を保持し、正常終了時とエラー時に解放する（[PR #44](https://github.com/TappoyGK/rin/pull/44)）。

- Debug VM で、既定の呼び出し深さ上限に達する前にスタックが溢れて異常終了する問題を修正した。2 MiB のスレッドスタック上で、通常の呼び出しと組み込み関数経由の呼び出しを深さ 200 まで実行でき、上限を超えると `TooDeep` を返す（[PR #42](https://github.com/TappoyGK/rin/pull/42)）。

- ホストが提供したソースの該当行にキャレットを付けて、エラーとともに表示する `Error::render` / `render_with` と、`fmt::Write` 向けの `render_to` / `render_with_to` を追加した。モジュールごとのソース選択と呼び出し履歴に対応し、タブ・全角文字・結合文字・絵文字の表示幅を考慮する（[PR #41](https://github.com/TappoyGK/rin/pull/41)）。

- 未定義名のエラーに近い名前の候補を、構造体の不存在フィールドやモジュールの不存在公開シンボルに利用可能な名前の一覧を付ける。Rust / C FFI のメッセージと表示文字列に含まれ、規則名・位置・呼び出し履歴も保持する（[PR #39](https://github.com/TappoyGK/rin/pull/39)）。

- エラーに仕様の規則名を付け、Rust の `Error::rule()` と C の `RinError.rule` から取得できるようにした。表示の先頭行にも `[rule.id]` を付ける。`Error` の各コンストラクタは第 1 引数に規則名を必須とする（[PR #38](https://github.com/TappoyGK/rin/pull/38)）。

- **Rust API の破壊的変更**：`Value` の公開ヴァリアントの中身を変更した。`Array` / `Tuple` は `Rc<ValueItems>`、`Struct` の `fields` は `Rc<ValueFields>`、`Some` / `Ok` / `Err` と `Variant` の子は `Rc<ValueChild>` を保持し、最後の所有者で解放する。構築には `Value::array(items)`・`tuple(items)`・`structure(name, fields)`・`variant(name, tag, data)`・`some(v)`・`ok(v)`・`err(v)` を使う。直接構築する場合は `Value::Some(Rc::new(v.into()))` のように所有型へ変換する。`Weak<Value>` も子を指す場合は `Weak<ValueChild>` となる（[PR #37](https://github.com/TappoyGK/rin/pull/37)）。
- 深い値の import / export、比較、表示、スキーマ検証、解放でネイティブスタックが溢れる問題を修正した。import の途中失敗時には生成途中の値を回収する（[PR #37](https://github.com/TappoyGK/rin/pull/37)）。
- `Value` / `ListNode` の `{:#?}` は浅い部分を字下げし、書式化の深さ上限に達した部分木をコンパクトに出力する。リストのセルも深さに含めるため、平らな長いリストも途中で切り替わる（[PR #37](https://github.com/TappoyGK/rin/pull/37)）。
- スキーマ検証で、値を消費しない参照循環を検出してエラーを返すようにした（[PR #37](https://github.com/TappoyGK/rin/pull/37)）。

- **Rust API の破壊的変更**：`Value::Fn` と `value::Closure` を削除した。対応する組み込み関数がない名前の `Value::Builtin` は、ホストからの取り込み時にエラーになる（[PR #4](https://github.com/TappoyGK/rin/pull/4)）。
- **Rust API の破壊的変更**：`rin::schema::TypeExpr` を非公開にした（[PR #34](https://github.com/TappoyGK/rin/pull/34)）。
- **C API の破壊的変更**：`RinEvalResult` の `error_message` / `error_line` / `error_col` と `rin_last_backtrace` を削除した。エラー情報は `rin_last_error` が返す `RinError` から取得する（[PR #36](https://github.com/TappoyGK/rin/pull/36)）。
- **Rust / C API の変更**：Rust の `Error` のコンストラクタは第1引数に規則名が必要になった。C の `RinError.rule` にも規則名を設定し、引数エラーには `host.argument` を付ける（[PR #38](https://github.com/TappoyGK/rin/pull/38)）。
- README に名前の由来・背景・想定用途を追加した（[PR #33](https://github.com/TappoyGK/rin/pull/33)）。
- 開発中の変更を固定の `release` ブランチへ集約し、`main` へのリリース PR とタグで公開する運用に変更した。CI を作業用とリリース用に分けた（[PR #13](https://github.com/TappoyGK/rin/pull/13)）。

## v0.1.0

- 純粋関数型 Lisp 系言語 Rin の字句解析・構文解析・評価器を実装し、`i64`・`f64`・`bool`・`char`・`string`・`tuple`・`struct`・`variant`・`option`・`result` に対応した。
- コンスセルによる不変連結リスト `list`（`O(1)` の `cons`・`car`・`cdr`・`len`・`is_empty`）と、連続配列 `array`（`list->array`・`array->list`）および高階リスト演算を追加した。
- ホスト提供型のモジュール機構（`import`・`export`・`load_module`・`call_in`）を追加し、ファイル I/O なしでモジュール分離と相互再帰を行えるようにした。
- 外部アロケータ境界 `Allocator`（`HeapAllocator`・`ArenaAllocator`）と、内部の参照カウント・フリーリスト再利用プール、および `steps`・`depth`・`memory_bytes` の資源上限を追加した。
- C FFI（`rin.h`）を追加し、値変換・`rin_call` / `rin_call_in` による直接呼び出し・構造体・バリアントの構築と読み取りに対応した。
- スキーマ検証言語（`Schema`・`rin_schema`・`rin_check`・`rin_check_str`）を追加し、`struct`（厳格）・`open_struct`（開放）・`defschema`（再帰参照）・`any` に対応した。
