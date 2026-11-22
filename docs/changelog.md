# 変更履歴

利用者に関わる変更をバージョンごとに記載し、関連する PR のリンクを付ける。イシューへのリンクは記載しない。

## 未リリース

- 組み込み関数の走査・比較・生成を、要素数や文字列長に応じてステップ数へ計上するようにした。同じ `steps` 上限でも、従来より早く `TooManySteps` になる場合がある。関数ごとの計上式と処理量を仕様に定めた（[PR #58](https://github.com/TappoyGK/rin/pull/58)）。

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
