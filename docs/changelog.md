# 変更履歴

利用者に関わる変更をバージョンごとに記載する。

## v0.1.1（未リリース）

- **Rust API の破壊的変更**：`Value` の公開ヴァリアントの中身を変更した。`Array` / `Tuple` は `Rc<ValueItems>`、`Struct` の `fields` は `Rc<ValueFields>`、`Some` / `Ok` / `Err` と `Variant` の子は `Rc<ValueChild>` を保持し、最後の所有者で解放する。構築には `Value::array(items)`・`tuple(items)`・`structure(name, fields)`・`variant(name, tag, data)`・`some(v)`・`ok(v)`・`err(v)` を使う。直接構築する場合は `Value::Some(Rc::new(v.into()))` のように所有型へ変換する。`Weak<Value>` も子を指す場合は `Weak<ValueChild>` となる。
- 深い値の import / export、比較、表示、スキーマ検証、解放でネイティブスタックが溢れる問題を修正した。import の途中失敗時には生成途中の値を回収する（[#31](https://github.com/TappoyGK/rin/issues/31)・[PR #37](https://github.com/TappoyGK/rin/pull/37)）。
- `Value` / `ListNode` の `{:#?}` は浅い部分を字下げし、書式化の深さ上限に達した部分木をコンパクトに出力する。リストのセルも深さに含めるため、平らな長いリストも途中で切り替わる。
- スキーマ検証で、値を消費しない参照循環を検出してエラーを返すようにした。

## v0.1.0

- 純粋関数型 Lisp 系言語 Rin の字句解析・構文解析・評価器を実装し、`i64`・`f64`・`bool`・`char`・`string`・`tuple`・`struct`・`variant`・`option`・`result` に対応した。
- コンスセルによる不変連結リスト `list`（`O(1)` の `cons`・`car`・`cdr`・`len`・`is_empty`）と、連続配列 `array`（`list->array`・`array->list`）および高階リスト演算を追加した。
- ホスト提供型のモジュール機構（`import`・`export`・`load_module`・`call_in`）を追加し、ファイル I/O なしでモジュール分離と相互再帰を行えるようにした。
- 外部アロケータ境界 `Allocator`（`HeapAllocator`・`ArenaAllocator`）と、内部の参照カウント・フリーリスト再利用プール、および `steps`・`depth`・`memory_bytes` の資源上限を追加した。
- C FFI（`rin.h`）を追加し、値変換・`rin_call` / `rin_call_in` による直接呼び出し・構造体・バリアントの構築と読み取りに対応した。
- スキーマ検証言語（`Schema`・`rin_schema`・`rin_check`・`rin_check_str`）を追加し、`struct`（厳格）・`open_struct`（開放）・`defschema`（再帰参照）・`any` に対応した。
