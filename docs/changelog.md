# 変更履歴

利用者に関わる変更をバージョンごとに記載し、関連する PR のリンクを付ける。イシューへのリンクは記載しない。

## 未リリース

- **破壊的変更（静的型付けへの移行）**：`defschema`・型注釈・`datum` 型と型パターン・モジュール修飾の構造体／バリアント構築とパターンを導入し、`defn` の型注釈を必須とし、実行前にゼロヒープの型検査を行う（[PR #83](https://github.com/TappoyGK/rin/pull/83)）。
- **破壊的変更（ホスト境界の型検証と型照会 API）**：スキーマ DSL の `any` を `datum` に置き換え、`bind` / `eval_with` 系 API に型指定と実行時検証（`[type.boundary]`）を導入した。関数シグネチャ・型照会 API と事前解決型ハンドル（`TypeHandle` / `RinType`）も追加した（[PR #83](https://github.com/TappoyGK/rin/pull/83)）。
- **破壊的変更（`eval_with` のレキシカルスコープ化と再定義の禁止）**：`eval_with` の初期束縛をレキシカルスコープに変更した。読み込み段階での同名再定義・組み込み名の上書き・ホストによるモジュールの再読み込み（`load_module` / `rin_load`）をエラーとし、失敗した評価の追加束縛はロールバックする（[PR #81](https://github.com/TappoyGK/rin/pull/81)）。
- 複数行文字列リテラル `"""..."""`（閉じ行基準のインデント除去と `${式}` 埋め込み対応）を追加した（[PR #79](https://github.com/TappoyGK/rin/pull/79)）。
- **破壊的変更（`text` 改名・テンプレート埋め込み・`f64` 表示統一）**：文字列型を `string` から `text` に改名し、`"..."` 内の `${式}` 埋め込み・添字・スライス・大小比較を追加した。`f64` の表示を Suz 共通の最短往復規則に統一した（[PR #78](https://github.com/TappoyGK/rin/pull/78)）。
- **破壊的変更（文字リテラル `\` 形式への移行）**：文字リテラルを `'a'` から `\a` / `\u{H}` / 名前つき文字形式に変更し、`'` を `quote` 専用とした。文字列エスケープも Suz と共通化した（[PR #77](https://github.com/TappoyGK/rin/pull/77)）。
- **破壊的変更（字句整理・`quote` / `quasiquote`・`ident`・`(tuple ...)`）**：リスト・構造体・`with` を空白区切りに、タプルを `(tuple ...)` に変更した。`quote` / `quasiquote`・正規 S 式構文・識別子値型 `ident` を追加し、値の表示をそのまま再入力できる形に統一した（[PR #75](https://github.com/TappoyGK/rin/pull/75)）。
- **破壊的変更（`Allocator` の `unsafe trait` 化）**：`Allocator` トレイトを `unsafe trait` に変更し、サイズ 0 確保を含む安全性契約を定めた（[PR #73](https://github.com/TappoyGK/rin/pull/73)）。
- **破壊的変更（ゼロヒープ化とホスト境界・C FFI 刷新）**：内部メモリ管理を 3 領域（永続・オブジェクト・作業）に移行し、システムヒープ確保 0 回で動作するようにした。Rust にゼロヒープ API（`Val` / `ValueRef` / `ErrorRef`）を追加し、C FFI もアロケータ配置・借用戻り値へ刷新した（[PR #70](https://github.com/TappoyGK/rin/pull/70)）。
- **破壊的変更（C FFI の読み込み／実行分離）**：C FFI のハンドルを `Rin` に改名し、`rin_build` と `rin_eval_with` を追加、凍結後専用の `rin_eval` と分離した（[PR #65](https://github.com/TappoyGK/rin/pull/65)）。
- **破壊的変更（大域環境の凍結）**：`Builder`（読み込み段階）と `Machine`（実行段階）を分離し、`def` / `defn` / `import` / `export` を読み込み段階のトップレベル専用とした（[PR #63](https://github.com/TappoyGK/rin/pull/63)）。
- `sin` / `cos` を決定的な自前実装に置き換え、`MathFma` 設定を追加した（[PR #62](https://github.com/TappoyGK/rin/pull/62)）。
- 組み込み関数の走査・比較・生成と `with` の複製をステップ上限に計上するようにした（[PR #58](https://github.com/TappoyGK/rin/pull/58)）。
- 決定性の規則（`sin` / `cos`、NaN、メモリ確保失敗、スタック保証範囲）を明文化した（[PR #57](https://github.com/TappoyGK/rin/pull/57)）。
- ソース整形 API `rin::fmt::format` と `rin fmt` CLI を追加した（[PR #54](https://github.com/TappoyGK/rin/pull/54)）。
- テスト構文（`deftest` / `check` / `check_eq`）と `rin test` CLI を追加し、`unicode-width` 依存を削除した（[PR #53](https://github.com/TappoyGK/rin/pull/53)）。
- **破壊的変更**：行コメントを `//` から `;` に変更した（[PR #49](https://github.com/TappoyGK/rin/pull/49)）。

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
