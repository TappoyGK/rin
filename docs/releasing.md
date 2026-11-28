# リリース手順

作業PRは `release/vVERSION` に集める。リリースブランチから `main` へのドラフトPRを早期に作り、作業PRの取り込みに合わせて更新する。最後のリリース準備PRでバージョンと変更履歴を確定する。公開には、マージ後の `main` のコミットから生成・検証した成果物（Rust クレートおよび静的ライブラリ `librin.a` + `rin/include/rin.h`）を使う。PRの区分は[PRの種類](pr.md#prの種類)を参照する。

> **移行注記**：v0.2.0 までは既存の `release` ブランチ（ドラフト PR #51）のまま完了し、v0.3.0 から版ごとの `release/vVERSION` ブランチへ切り替える。移行期間中は CI も `release` と `release/**` の両方を受け付ける。

## 毎回の進め方

1. 最新の `main` から `release/vVERSION` を作り、push する。
2. `main` との差分ができた時点で、`main` 向けのリリースPRをドラフトとして作る。差分がない間は作成できないため、最初の作業PRを取り込んだ直後に作成する。
3. [PRの作業手順](pr.md)に従って作業PRを取り込む。
4. 公開対象が揃ったら、スタックの最後のリリース準備PRでバージョンと変更履歴を確定する。取り込む全作業PRと変更履歴を照合し、利用者向けの変更が欠けていないことを確認する。
5. 準備PRを取り込んだら、リリースPRのタイトル・説明・関連イシュー（`Closes #番号`）を確定し、ドラフトを解除する。人間がレビューとCIの結果確認を行い、merge commit で `main` へ取り込む。
6. マージ後の `main` のコミットに `vVERSION` タグを付与して push・公開する。

## 1. リリースブランチの作成

最新の `main` から `release/vVERSION` を作成して push する。1つのリリースブランチに、複数の作業PRを取り込める。

`main` との差分ができたら、次のコマンドでドラフトのリリースPRを作成する。説明は実際の改行を持つファイルに用意する。既存のリリースPRがある場合は、それを更新する。

```sh
gh pr create --base main --head release/vVERSION --draft \
    --title "release: vVERSION" --body-file /path/to/release-pr.md
```

### ブランチ保護

`release/*`（v0.2.0 移行期間中は `release`）はPR経由で変更し、最新の取り込み先を反映した `work checks` の成功を必須とする。`main` はリリースPR経由で変更し、最新の `main` を反映した `release checks` の成功を必須とする。両方の保護で force push とブランチ削除を禁止する。

## 2. 作業PRの取り込み

作業PRの作成・レビュー・スタック管理は[PRの作業手順](pr.md)に従う。

## 3. リリース準備とリリースPR

### バージョンと変更履歴

リリース準備PRは、そのリリースで `release/*` に最後に取り込むPRとする。公開対象の作業PRを取り込んだ後、リリース準備用のブランチでバージョンと変更履歴を確定する。[作業PRのスタック](pr.md#作業prのスタック)の最後に積み、準備PRの後に変更が追加された場合は、その上に載せ直して内容を更新する。

コミット・レビュー・マージは[コミットとレビュー対応](pr.md#コミットとレビュー対応)に従う。CIの設定は[ワークフロー](../.github/workflows/ci.yml)を参照する。

1. `rin/Cargo.toml` と `rin-cli/Cargo.toml` の `version` を `VERSION` に更新する（`Cargo.lock` も同期する）。
2. [変更履歴](changelog.md)に `## vVERSION` を確定し、取り込む全作業PRの利用者向けの変更と PR リンク（`（[PR #番号](...)）`）が漏れていないか照合する。変更履歴にはイシューへのリンクを記載しない。

### 全体の検査とレビュー

作成済みの `main` 向けリリースPRは、準備中はドラフトを維持し、作業PRをリリースブランチへ取り込むことで差分を更新する。最後の準備PRを取り込んだ後、説明を確定した公開内容に書き直し、関連イシューの `Closes #番号` を揃えてドラフトを解除する。古い取り込み順や準備中の記述を残さない。

人間が差分と変更後の全体をレビューする。最新の `main` をリリースブランチへ取り込む必要がある場合も、取り込み用の作業PRを使う。全検査に成功した状態で、人間がリリースPRをマージする。作業PR単位のコミットを保つため、リリースPRには merge commit を使う。修正が必要な場合は、リリースブランチ向けの作業PRを追加する。

### CIの実行条件

| 起動条件 | 必須チェック | 検査 |
|---|---|---|
| 作業PR → `release/*`（移行中は `release`） | `work checks` | 書式、Clippy（`--no-default-features` 含む）、`rustdoc`、決定性、通常テスト |
| `release/*`（移行中は `release`）への push | `work checks` | `rustdoc`、決定性、通常テスト |
| リリースPR → `main` | `release checks` | 全検査（debug / release / `--no-default-features`）、決定性、`librin.a` 生成確認 |
| `main` への push | `release checks` | 全検査（debug / release / `--no-default-features`）、決定性、`librin.a` 生成確認 |
| 手動実行 | `release checks` | 選択したブランチで全検査 |

手動実行と新規ブランチ作成時は、Markdown のみの変更でも検査を省略しない。変更ファイルによる検査省略の条件は[判定スクリプト](../scripts/ci-changes.py)を参照する。

## 4. タグと公開

マージによる `main` への push で CI（`release checks` および Linux との決定性比較）が成功したことを確認したうえで、`main` に `vVERSION` タグを付与して push・公開する。

```sh
git tag -a vVERSION -m "release: vVERSION"
git push origin vVERSION
```

## ローカルでの検査

リポジトリのルートで次を実行し、すべて警告・失敗なしで通ることを確認する。

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
sh scripts/check-determinism.sh
python3 scripts/test-ci-changes.py
```

公開用の静的ライブラリ（`target/release/librin.a`）と C ヘッダ（`rin/include/rin.h`）が揃っていることも確認する。
