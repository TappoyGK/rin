# 仕様の執筆・レビュー基準

言語の目的は[言語仕様](spec.md)を正本とする。この文書は、その規定を欠落させずに仕様を記述・整理するための基準である。

Suz / Rin 共通の原則（処理量・資源消費の定型文、正本配置の判断順、規範と例の区別、規則IDの運用原則、編集と仕様変更の区別、機械・人による確認項目）は [TappoyGK/suzrin の共通基準](https://github.com/TappoyGK/suzrin/blob/main/docs/spec-writing-common.md) を正本とする。以下は Rin 固有の配置・規則ID・確認事項を定める。

## 1. 処理量と資源消費の規定（Rin 固有）

Rin では [言語仕様](spec.md) の `[eval.cost-resources]` に従い、ステップ上限・呼び出し深さ上限・メモリ上限（3 領域 `PermArena` / `ObjHeap` / `WorkArena` の論理使用量）およびステップに計上しない処理（構文解析・型検査・コンパイル・解放・診断）の扱いを明確にする。

記述の定型文（「処理量の上限を定める。」「処理量の上限を定めない。」）と 4 項目の表の書き方は [共通基準 §1](https://github.com/TappoyGK/suzrin/blob/main/docs/spec-writing-common.md#1-処理量と資源消費の規定) に従う。Rin では、共通基準の「外部処理」はホストから渡された `Allocator`・OS・ソース取得コールバック・出力先の処理を指し、現実装の技術的な根拠は [内部構成](architecture.md) へ置く。また、Rin の `[eval.cost-resources]` 以降の 4 項目の表では、「外部処理」の代わりに「確保」の行を置く。

## 2. 正本を置く場所

| 内容 | 正本 |
|---|---|
| 文法・型・値・評価規則・資源制限・決定性 | [言語仕様](spec.md) |
| 値検証用スキーマ言語の文法と規則 | [スキーマ仕様](schema.md) |
| Rust API および C FFI API・所有権・エラー記録 | [API 仕様](api.md) |
| 整形規則・CLI・AST 保存 | [フォーマッタ仕様](fmt.md) |
| テストの実行・CLI・JSON 出力 | [テストツール仕様](testing.md) |
| `sin`・`cos` の計算手順・定数・テスト値（Suz との共有正本） | [数学関数の決定的な計算手順](math-spec.md) |
| 受理・拒否・評価結果の具体例 | [仕様例集](spec-cases.md) |
| 評価環境・3 領域メモリ管理・コンパイル検査の内部設計 | [内部構成](architecture.md) |
| ビルド・利用方法・テスト実行方法 | [インタプリタ](../rin/README.md) |

配置の判断順および規範・要約・理由・例の区別は [共通基準 §2・§3](https://github.com/TappoyGK/suzrin/blob/main/docs/spec-writing-common.md#2-正本配置の判断順) に従う。

## 3. 規則IDと参照

| 接頭辞 | 定義する文書と役割 |
|---|---|
| `src.`・`type.`・`value.`・`expr.`・`pat.`・`op.`・`fn.`・`eval.`・`env.`・`compile.`・`error.`・`host.` | [言語仕様](spec.md)：字句・構文・型・値・式・パターン・演算・組み込み関数・評価・環境・コンパイル・エラー・ホスト境界 |
| `schema.` | [スキーマ仕様](schema.md)：スキーマ検証 |
| `fmt.` | [フォーマッタ仕様](fmt.md)：整形・AST 保存 |
| `test.` | [言語仕様](spec.md)（`[test.deftest]`・`[test.check]`・`[test.result]`）および [テストツール仕様](testing.md)（`[test.modules]`） |

規則IDの改名・移動・参照の扱いは [共通基準 §4](https://github.com/TappoyGK/suzrin/blob/main/docs/spec-writing-common.md#4-規則idの運用原則) に従う。

## 4. 編集と仕様変更の区別・確認

編集と仕様変更の区別、および機械・人による確認項目は [共通基準 §5・§6](https://github.com/TappoyGK/suzrin/blob/main/docs/spec-writing-common.md#5-編集と仕様変更の区別) に従う。Rin では機械的な確認として `cargo test -p rin --locked --test spec_cases --test error_rules --test formatter` で規則IDの存在・仕様例集の件数と評価結果・フォーマッタの冪等性を検証する。
