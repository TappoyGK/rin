# Rin インタプリタ

Rin 言語の純粋評価器（パーサ・評価器・資源制限）を実装する Rust クレートである。

## テスト

```sh
cargo test
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
