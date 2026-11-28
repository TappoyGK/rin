# リリース手順

作業 PR は固定の `release` ブランチに集め、リリース準備でバージョンと変更履歴を確定したうえで `main` へ取り込む。`release` ブランチは削除せず使い回し、バージョンの区切りは `main` 上の `vVERSION` タグで管理する。公開には、マージ後の `main` のコミットから生成・検証した成果物（Rust クレートおよび静的ライブラリ `librin.a` + `rin/include/rin.h`）を使う。

## 1. リリース運用と作業 PR

各バージョンの開発を始めたら、最初に `release` → `main` のリリース PR を **ドラフト** で作る。作業 PR を `release` に取り込むと、同じリリース PR の差分も更新されるため、準備中に作り直さない。取り込む機能・破壊的変更・対象イシューに合わせて本文を更新する。

リリース PR をマージしたら、その PR は完了とする。次のバージョンでは、同じ `release` ブランチから新しいドラフト PR を作る。PR のマージとタグの公開はユーザーが行う。

作業 PR の作成・レビュー・スタック管理は [PR の作業手順](pr.md) に従う。

## 2. リリース準備

公開対象が揃ったら、`release` から準備ブランチを作り、バージョンと変更履歴を確定する準備 PR を `release` 向けに作成する。検証後、ユーザーが準備 PR をマージするまでリリース PR はドラフトのままにする。

1. `rin/Cargo.toml` と `rin-cli/Cargo.toml` の `version` を `VERSION` に更新する（`Cargo.lock` も同期する）。
2. [変更履歴](changelog.md) に `## vVERSION` を確定し、取り込む作業 PR のリンク（`（[PR #番号](...)）`）が漏れていないか照合する。変更履歴にはイシューへのリンクを記載しない。
3. 次の検査を実行し、すべて警告・失敗なしで通ることを確認する。

```sh
cargo fmt --all -- --check
cargo clippy --workspace --locked --all-targets -- -D warnings
cargo check -p rin --no-default-features --locked
cargo clippy -p rin --no-default-features --locked --all-targets -- -D warnings
cargo test -p rin --no-default-features --locked --all-targets
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --locked --no-deps
cargo test --workspace --locked
cargo test --workspace --locked --release
cargo build --release --workspace --locked
```

4. 公開用の静的ライブラリ（`target/release/librin.a`）と C ヘッダ（`rin/include/rin.h`）が揃っていることを確認する。

## 3. マージとタグ公開

1. 準備 PR を取り込んだ後、既存のリリース PR に `Closes #番号` を揃え、最新の `release checks` 通過を確認してドラフトを解除する。レビュー後、ユーザーがマージする。
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
