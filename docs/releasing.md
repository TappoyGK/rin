# リリース手順

作業 PR は `release/vVERSION` に集め、リリース準備でバージョンと変更履歴を確定したうえで `main` へ取り込む。公開には、マージ後の `main` のコミットから生成・検証した成果物（Rust クレートおよび静的ライブラリ `librin.a` + `rin/include/rin.h`）を使う。

## 1. リリースブランチと作業 PR

1. 最新の `main` から `release/vVERSION` を作成する。
2. 作業ブランチは `release/vVERSION` から作成し、仕様（`docs/spec.md` / `docs/schema.md` / `docs/api.md`）・実装（`rin/src/`）・仕様例（`docs/spec-cases/`）・テストを揃えて PR を出す。
3. コミットメッセージは `docs:` / `feat:` / `fix:` などの接頭辞に続けて日本語で書く。

## 2. リリース準備

公開対象が揃ったら、最終確認とバージョン確定を行う。

1. `rin/Cargo.toml` の `version` を `VERSION` に更新する。
2. [変更履歴](changelog.md) に `## vVERSION` を確定し、取り込む変更が漏れていないか照合する。
3. 次の検査を実行し、すべて警告・失敗なしで通ることを確認する。

```sh
cargo test --workspace
cargo build --release --workspace
```

4. 公開用の静的ライブラリ（`target/release/librin.a`）と C ヘッダ（`rin/include/rin.h`）が揃っていることを確認する。

## 3. マージとタグ公開

1. `release/vVERSION` から `main` へのリリース PR をレビュー・マージする。
2. マージ後の `main` に `vVERSION` タグを付与して公開する。

```sh
git tag -a vVERSION -m "release: vVERSION"
```
