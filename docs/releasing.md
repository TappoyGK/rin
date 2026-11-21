# リリース手順

作業 PR は固定の `release` ブランチに集め、リリース準備でバージョンと変更履歴を確定したうえで `main` へ取り込む。`release` ブランチは削除せず使い回し、バージョンの区切りは `main` 上の `vVERSION` タグで管理する。公開には、マージ後の `main` のコミットから生成・検証した成果物（Rust クレートおよび静的ライブラリ `librin.a` + `rin/include/rin.h`）を使う。

## 1. リリース運用と作業 PR

1. 作業ブランチは `release` から作成し、仕様（`docs/spec.md` / `docs/schema.md` / `docs/api.md`）・実装（`rin/src/`）・仕様例（`docs/spec-cases/`）・テストを揃えて `release` 向けの PR を出す。
2. コミットメッセージは `docs:` / `feat:` / `fix:` などの接頭辞に続けて日本語で書く。
3. 作業 PR 本文では関連イシューを `Refs #番号` 等で紐づけ、自動クローズ（`Closes #番号`）は `main` 向けリリース PR でまとめて行う。
4. 作業 PR は `work checks` の通過とレビュー後に squash merge で `release` へ取り込む。

## 2. リリース準備

公開対象が揃ったら、`release` 上で最終確認とバージョン確定を行う準備 PR を作成・取り込む。

1. `rin/Cargo.toml` の `version` を `VERSION` に更新する（`Cargo.lock` も同期する）。
2. [変更履歴](changelog.md) に `## vVERSION` を確定し、取り込む作業 PR とイシューのリンク（`（[#イシュー](...)・[#PR](...)）`）が漏れていないか照合する。
3. 次の検査を実行し、すべて警告・失敗なしで通ることを確認する。

```sh
cargo fmt --all -- --check
cargo clippy --workspace --locked --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --locked --no-deps
cargo test --workspace --locked --release
cargo build --release --workspace --locked
```

4. 公開用の静的ライブラリ（`target/release/librin.a`）と C ヘッダ（`rin/include/rin.h`）が揃っていることを確認する。

## 3. マージとタグ公開

1. `release` から `main` へのリリース PR に `Closes #番号` を揃えてレビュー・`release checks` 通過後にマージする。
   - **重要**：`release` ブランチを継続して使い回すため、`release` → `main` のマージは必ず **merge commit** を使う（squash merge しない）。
2. マージ後の `main` に `vVERSION` タグを付与して push・公開する。

```sh
git tag -a vVERSION -m "release: vVERSION"
git push origin vVERSION
```

3. `main` 側で直接修正が入った場合を除き、`release` はそのまま次の開発の基点として使い回す（`main` に直接コミットが入った場合のみ `main` を `release` へ取り込む）。

## 4. CI の実行条件

| 起動条件 | 必須チェック | 検査 |
|---|---|---|
| 作業 PR → `release` | `work checks` | 書式、Clippy、`rustdoc`、通常テスト |
| `release` への push | `work checks` | `rustdoc`、通常テスト |
| リリース PR → `main` | `release checks` | 全検査（debug / release テスト）、`librin.a` 生成確認 |
| `main` への push | `release checks` | 全検査（debug / release テスト）、`librin.a` 生成確認 |
| 手動実行 | `release checks` | 選択したブランチで全検査 |
