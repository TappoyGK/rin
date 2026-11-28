# ソースの開発規約

インタプリタ、CLI、C FFI、スクリプトとテストの規約を定める。仕様文書の書き方は[仕様の執筆・レビュー基準](spec-writing.md)、PRの作成・レビューは[PRの作業手順](pr.md)、公開までの操作は[リリース手順](releasing.md)に従う。

Suz / Rin 共通の原則（命名・責務分離、コメントとrustdoc、ターミナル出力と診断、単体テストの配置規約、検証方針、整形・リントの方針）は [TappoyGK/suzrin の共通規約](https://github.com/TappoyGK/suzrin/blob/main/docs/development-common.md) を正本とする。以下は Rin 固有の配置・制約・検証コマンドを定める。

## 配置・命名（Rin 固有）

| 場所 | 置くもの |
|---|---|
| `rin/src/` | Rin インタプリタ・型検査器・C FFI 実装と、内部を直接検証する単体テスト |
| `rin/include/` | C FFI 公開ヘッダ（`rin.h`） |
| `rin/tests/` | 公開 API・仕様例集・メモリ・ゼロヒープ・C FFI リンクなどの統合テスト |
| `rin-cli/src/` | `rin` CLI（`rin fmt`・`rin test`） |
| `scripts/` | 決定性検証・CI 変更分類など開発・検査用スクリプト |
| `docs/spec-cases/` | 読んで理解でき、継続して評価を検証する Rin の仕様例 |

C FFI の外部シンボルには `rin_`（型は `Rin` / `RIN_`）を付ける。その他の命名・責務分離は [共通規約](https://github.com/TappoyGK/suzrin/blob/main/docs/development-common.md#命名責務) に従う。

## メモリ管理・ゼロヒープ・決定性

- コア評価器・型検査器・ゼロヒープホスト API（`--no-default-features`）および C FFI では、システムヒープを確保せずホストから渡された `Allocator` 上の 3 領域（`PermArena`・`ObjHeap`・`WorkArena`）だけで完結させる。
- 評価・型検査・解放の深さ依存パスはネイティブスタックに依存させず、反復処理または明示的スタックで実装する。
- 評価結果・エラー・表示順は Debug / Release・プラットフォーム間で一致させ、`sh scripts/check-determinism.sh` で検証する。

## コメント・診断・テスト配置

コメント・rustdoc、ターミナル出力と診断、単体テストの配置規約は [共通規約](https://github.com/TappoyGK/suzrin/blob/main/docs/development-common.md#コメントとrustdoc) に従う。

## 整形・リント・確認

```sh
cargo fmt --all -- --check
cargo clippy --workspace --locked --all-targets -- -D warnings
cargo clippy -p rin --no-default-features --locked --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --locked --no-deps
cargo test --workspace --locked
cargo test -p rin --no-default-features --locked --all-targets
sh scripts/check-determinism.sh
python3 scripts/test-ci-changes.py
git diff --check
```

作業中は変更に関係するテストで確認し、PR の CI では `work checks` を実行する。リリース時は[リリース手順](releasing.md)に従って全検査を行う。
