# 変更履歴

利用者に関わる変更をバージョンごとに記載する。

## v0.1.1

- 評価器をスロット解決付きのバイトコード VM へ移行し、末尾呼び出し最適化（`depth` を消費しない末尾再帰・末尾呼び出し）を追加した。
- **破壊的変更**：ホスト境界で受け渡しできなかった `Value::Fn` および `value::Closure` を公開 API から削除し、`Value::Builtin` で未知の組み込み関数名を受け取った場合はエラーにするようにした。

## v0.1.0

- 純粋関数型 Lisp 系言語 Rin の字句解析・構文解析・評価器を実装し、`i64`・`f64`・`bool`・`char`・`string`・`tuple`・`struct`・`variant`・`option`・`result` に対応した。
- コンスセルによる不変連結リスト `list`（`O(1)` の `cons`・`car`・`cdr`・`len`・`is_empty`）と、連続配列 `array`（`list->array`・`array->list`）および高階リスト演算を追加した。
- ホスト提供型のモジュール機構（`import`・`export`・`load_module`・`call_in`）を追加し、ファイル I/O なしでモジュール分離と相互再帰を行えるようにした。
- 外部アロケータ境界 `Allocator`（`HeapAllocator`・`ArenaAllocator`）と、内部の参照カウント・フリーリスト再利用プール、および `steps`・`depth`・`memory_bytes` の資源上限を追加した。
- C FFI（`rin.h`）を追加し、値変換・`rin_call` / `rin_call_in` による直接呼び出し・構造体・バリアントの構築と読み取りに対応した。
- スキーマ検証言語（`Schema`・`rin_schema`・`rin_check`・`rin_check_str`）を追加し、`struct`（厳格）・`open_struct`（開放）・`defschema`（再帰参照）・`any` に対応した。
