# 文書目録

## 仕様

| 文書 | 内容 |
|---|---|
| [言語仕様](spec.md) | Rin の文法・型・値・評価規則・資源制限 |
| [スキーマ仕様](schema.md) | 値検証用スキーマ言語の文法と規則 |
| [API 仕様](api.md) | Rust API および C FFI API の仕様 |
| [フォーマッタ仕様](fmt.md) | 整形規則・CLI・ASTの保存 |
| [テストツール仕様](testing.md) | テスト実行・CLI・JSON出力 |
| [数学関数の決定的な計算手順](math-spec.md) | `sin`・`cos` の手順・定数・テスト値（Suz との共有正本） |
| [仕様例集](spec-cases.md) | 言語仕様の具体例と期待結果 |

## 開発・運用

| 文書 | 内容 |
|---|---|
| [内部構成](architecture.md) | 評価環境（`Builder`/`Machine`）・3 領域メモリ管理・コンパイル検査・名前解決の内部設計 |
| [インタプリタ](../rin/README.md) | ビルド・利用方法・テスト方法 |
| [開発への参加](../CONTRIBUTING.md) | イシューの書き方と更新ルール（正本：[TappoyGK/suzrin](https://github.com/TappoyGK/suzrin)） |
| [PR の作業手順](pr.md) | 作業 PR・スタック・レビュー対応の手順（正本：[TappoyGK/suzrin](https://github.com/TappoyGK/suzrin)） |
| [仕様の執筆・レビュー基準](spec-writing.md) | 仕様文書の正本配置・規則 ID・確認基準（共通正本：[TappoyGK/suzrin](https://github.com/TappoyGK/suzrin)） |
| [ソースの開発規約](development.md) | 配置・命名・コメント・診断・テスト・リント規約（共通正本：[TappoyGK/suzrin](https://github.com/TappoyGK/suzrin)） |

### 共有文書の更新

- `CONTRIBUTING.md`・`.github/ISSUE_TEMPLATE.md`・`.github/PULL_REQUEST_TEMPLATE.md`・`docs/pr.md` の正本は [TappoyGK/suzrin](https://github.com/TappoyGK/suzrin) にあり、変更は `suzrin` 側で行ってから `suzrin` の `scripts/sync.py` で各リポジトリへ反映する。
- `docs/spec-writing.md` と `docs/development.md` の共通部分を変更するときも、[TappoyGK/suzrin](https://github.com/TappoyGK/suzrin) の文書（`docs/spec-writing-common.md`・`docs/development-common.md`）を更新する。

## リリース

| 文書 | 内容 |
|---|---|
| [リリース手順](releasing.md) | 検査・成果物生成・公開手順 |
| [変更履歴](changelog.md) | バージョンごとの利用者向け変更点 |
