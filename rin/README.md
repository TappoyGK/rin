# Rin インタプリタ

Rin 言語の純粋評価器（パーサ・評価器・資源制限）を実装する Rust クレートである。

## テスト

```sh
cargo test
```

決定性の検証では、仕様例と各ステップ上限での評価結果を別プロセスで比較する。次のコマンドはDebug / Releaseの値・エラー・表示も照合する。CIではmacOSとLinuxの出力も比較する。

```sh
sh scripts/check-determinism.sh
```

## Modのテスト

```sh
cargo run -p rin-cli -- test --module-path ./modules
```

テストファイルの書き方とオプションは[テストツール仕様](../docs/testing.md)を参照。

## ソースの整形

```sh
cargo run -p rin-cli -- fmt --check
cargo run -p rin-cli -- fmt path/to/source.rin
```

[フォーマッタ仕様](../docs/fmt.md)を参照。

## ソース構成

| 対象 | 主なファイル |
|---|---|
| 3 領域メモリ管理 | `src/mem/` に `AllocCore`・`PermArena`・`ObjHeap`・`WorkArena`・自前コンテナ（`RawVec`・`RawStr`・`RawMap`・`WorkBox`）と `MemoryConfig` を置く |
| ゼロヒープ値・エラー境界 | `src/value_ref.rs` に `Val`・`ValueRef`・各種借用ビュー、`src/error/store.rs` にインライン `ErrorStore` と `ErrorRef`、`src/eval/host_api.rs` に `val_*` / `eval_ref` 等の公開メソッド、`src/eval/host_vals.rs` にその内部の値構築を置く |
| クレート内テスト | `src/tests.rs` で登録し、`src/tests/` 以下で評価・コレクション・FFI・モジュール・スキーマ・資源制限・診断を検証する |
| スキーマ | `src/schema.rs` に公開型を置き、`schema/` 以下で型表現・解析・照合・複製・表示・解放を扱う |
| フォーマッタ | `src/fmt/mod.rs` が整形とAST保存検査を組み立てる。`fmt/syntax.rs` が書式を選び、`fmt/cst/` が構文木の構築・幅計算・出力を担う |
| パーサ | `src/parser.rs` に入口と解析ループを置き、`parser/` 以下で途中状態・式の開始・解析の再開・パターンを扱う |
| コンパイラ | `src/compiler.rs` に入口と命令出力を置く。`compiler/scope.rs` が名前解決、`shallow.rs` が浅い構文、`lower.rs` と `steps.rs` が深い構文の反復処理を担う |
| 組み込み関数 | `src/eval/builtins.rs` が振り分けを行う。`builtins/collections.rs` にコレクション操作、`callbacks.rs` にRinの関数を呼び戻す操作を置く |
| 評価環境 | `src/eval/mod.rs` に共有する状態を置く。`machine.rs` がホストAPI、`modules.rs` がモジュール管理、`runner.rs` が評価中の状態操作を担う |
| VM | `src/eval/vm.rs` がフレームを実行し、`vm/` 以下で命令ループ・呼び出し・パターン照合・エラー履歴を扱う |

コンパイラの浅い構文の処理と、深い構文を扱う作業スタックは、速度とスタック使用量のために分けている。VMの命令ループは `vm/execute.rs` にまとめ、debug/releaseそれぞれの呼び出し経路を保つ。
