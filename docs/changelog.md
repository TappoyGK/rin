# 変更履歴

利用者に関わる変更をバージョンごとに記載し、関連する PR のリンクを付ける。イシューへのリンクは記載しない。

## v0.1.1（未リリース）

- 関数のローカル値をキャプチャしたクロージャを末尾の位置で呼ぶと、値が先に解放されて panic する問題を修正した。呼び出し先の実行中はキャプチャした値を保持し、正常終了時とエラー時に解放する（[PR #44](https://github.com/TappoyGK/rin/pull/44)）。

- Debug VM で、既定の呼び出し深さ上限に達する前にスタックが溢れて異常終了する問題を修正した。2 MiB のスレッドスタック上で、通常の呼び出しと組み込み関数経由の呼び出しを深さ 200 まで実行でき、上限を超えると `TooDeep` を返す（[PR #42](https://github.com/TappoyGK/rin/pull/42)）。

- ホストが提供したソースの該当行にキャレットを付けて、エラーとともに表示する `Error::render` / `render_with` と、`fmt::Write` 向けの `render_to` / `render_with_to` を追加した。モジュールごとのソース選択と呼び出し履歴に対応し、タブ・全角文字・結合文字・絵文字の表示幅を考慮する（[PR #41](https://github.com/TappoyGK/rin/pull/41)）。

- 未定義名のエラーに近い名前の候補を、構造体の不存在フィールドやモジュールの不存在公開シンボルに利用可能な名前の一覧を付ける。Rust / C FFI のメッセージと表示文字列に含まれ、規則名・位置・呼び出し履歴も保持する（[PR #39](https://github.com/TappoyGK/rin/pull/39)）。

- エラーに仕様の規則名を付け、Rust の `Error::rule()` と C の `RinError.rule` から取得できるようにした。表示の先頭行にも `[rule.id]` を付ける。`Error` の各コンストラクタは第 1 引数に規則名を必須とする（[PR #38](https://github.com/TappoyGK/rin/pull/38)）。

- **Rust API の破壊的変更**：`Value` の公開ヴァリアントの中身を変更した。`Array` / `Tuple` は `Rc<ValueItems>`、`Struct` の `fields` は `Rc<ValueFields>`、`Some` / `Ok` / `Err` と `Variant` の子は `Rc<ValueChild>` を保持し、最後の所有者で解放する。構築には `Value::array(items)`・`tuple(items)`・`structure(name, fields)`・`variant(name, tag, data)`・`some(v)`・`ok(v)`・`err(v)` を使う。直接構築する場合は `Value::Some(Rc::new(v.into()))` のように所有型へ変換する。`Weak<Value>` も子を指す場合は `Weak<ValueChild>` となる。
- 深い値の import / export、比較、表示、スキーマ検証、解放でネイティブスタックが溢れる問題を修正した。import の途中失敗時には生成途中の値を回収する（[PR #37](https://github.com/TappoyGK/rin/pull/37)）。
- `Value` / `ListNode` の `{:#?}` は浅い部分を字下げし、書式化の深さ上限に達した部分木をコンパクトに出力する。リストのセルも深さに含めるため、平らな長いリストも途中で切り替わる。
- スキーマ検証で、値を消費しない参照循環を検出してエラーを返すようにした。

## v0.1.0

- 純粋関数型 Lisp 系言語 Rin の字句解析・構文解析・評価器を実装し、`i64`・`f64`・`bool`・`char`・`string`・`tuple`・`struct`・`variant`・`option`・`result` に対応した。
- コンスセルによる不変連結リスト `list`（`O(1)` の `cons`・`car`・`cdr`・`len`・`is_empty`）と、連続配列 `array`（`list->array`・`array->list`）および高階リスト演算を追加した。
- ホスト提供型のモジュール機構（`import`・`export`・`load_module`・`call_in`）を追加し、ファイル I/O なしでモジュール分離と相互再帰を行えるようにした。
- 外部アロケータ境界 `Allocator`（`HeapAllocator`・`ArenaAllocator`）と、内部の参照カウント・フリーリスト再利用プール、および `steps`・`depth`・`memory_bytes` の資源上限を追加した。
- C FFI（`rin.h`）を追加し、値変換・`rin_call` / `rin_call_in` による直接呼び出し・構造体・バリアントの構築と読み取りに対応した。
- スキーマ検証言語（`Schema`・`rin_schema`・`rin_check`・`rin_check_str`）を追加し、`struct`（厳格）・`open_struct`（開放）・`defschema`（再帰参照）・`any` に対応した。
