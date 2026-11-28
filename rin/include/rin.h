#ifndef RIN_H
#define RIN_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef enum RinTag {
    RIN_TAG_VOID = 0,
    RIN_TAG_INT = 1,
    RIN_TAG_FLOAT = 2,
    RIN_TAG_BOOL = 3,
    RIN_TAG_CHAR = 4,
    RIN_TAG_TEXT = 5,
    RIN_TAG_LIST = 6,
    RIN_TAG_ARRAY = 7,
    RIN_TAG_TUPLE = 8,
    RIN_TAG_STRUCT = 9,
    RIN_TAG_VARIANT = 10,
    RIN_TAG_NONE = 11,
    RIN_TAG_SOME = 12,
    RIN_TAG_OK = 13,
    RIN_TAG_ERR = 14,
    RIN_TAG_FN = 15,
    RIN_TAG_IDENT = 16,
} RinTag;

typedef struct RinLimits {
    size_t steps;
    size_t depth;
    size_t memory_bytes;
} RinLimits;

/* 各領域のブロック拡張設定。
 * 内部で initial_block は 256 以上に、max_block は initial_block 以上に正規化される。
 */
typedef struct RinRegionConfig {
    size_t initial_block;
    size_t max_block;
} RinRegionConfig;

typedef struct RinMemoryConfig {
    RinRegionConfig perm;
    RinRegionConfig obj;
    RinRegionConfig work;
} RinMemoryConfig;

typedef enum RinMathFma {
    RIN_MATH_FMA_STRICT = 0,
    RIN_MATH_FMA_FUSED = 1
} RinMathFma;

/* Rin が利用する外部メモリアロケータ。
 * - alloc == NULL の場合は、dealloc の有無によらず標準ヒープ（HeapAllocator）で確保・解放する（dealloc は呼ばれない）。
 * - alloc != NULL の場合は、成功時に align に整列した少なくとも size バイトの読み書き可能な有効領域を返し、
 *   同時に生存する他の確保領域と重複させず、対応する dealloc または rin_free まで有効に保つこと。
 * - dealloc == NULL（alloc != NULL）の場合は個別解放を省略する（アリーナ用途）。その場合も生存中の領域を早期に再利用してはならない。
 * - dealloc に渡される ptr・size・align は確保時と同じ値である。
 * - size == 0 の要求では alloc / dealloc のいずれも呼び出さない。
 * - ctx と alloc / dealloc が指す関数は、rin_new（または rin_new_with_config）の呼び出しから
 *   rin_free が戻るまで有効に保つこと（rin_free は Rin 本体の解放にも dealloc を使用する）。
 */
typedef struct RinAllocator {
    void *ctx;
    uint8_t *(*alloc)(void *ctx, size_t size, size_t align);
    void (*dealloc)(void *ctx, uint8_t *ptr, size_t size, size_t align);
} RinAllocator;

typedef struct Rin Rin;
typedef struct RinValue RinValue;
typedef struct RinSchema RinSchema;

typedef enum RinErrorKind {
    RIN_ERR_LEX = 0,
    RIN_ERR_PARSE = 1,
    RIN_ERR_RUNTIME = 2,
    RIN_ERR_TOO_MANY_STEPS = 3,
    RIN_ERR_TOO_DEEP = 4,
    RIN_ERR_TOO_MUCH_MEMORY = 5,
    RIN_ERR_HOST = 6,
    RIN_ERR_TYPE = 7
} RinErrorKind;

/* 文字列は UTF-8・NUL 終端。Rin が所有し、ホストは解放しない。
 * 各文字列は固定長インライン領域（NUL 終端込みの上限: rule 64 / message 1024 /
 * module 128 / backtrace 2048 / text 3200 バイト）へ格納され、超過時は UTF-8 文字境界で切り詰められる。
 * 履歴が長い場合は backtrace（最大 2047 バイト）が text（最大 3199 バイト）より手前で切れることがある。
 */
typedef struct RinError {
    RinErrorKind kind;
    const char *message;   /* 常に非 NULL。見出しの本文。 */
    const char *module;    /* モジュール外なら NULL。 */
    const char *rule;      /* 根拠となる仕様の規則名。非 NULL、角括弧なし。 */
    size_t line;           /* 1 始まり。位置がなければ 0。 */
    size_t col;            /* 1 始まり。位置がなければ 0。 */
    const char *backtrace; /* 履歴がなければ NULL。 */
    const char *text;      /* 常に非 NULL。見出しと履歴の全体。 */
} RinError;

/* rin_eval_with に渡す 1 件の初期束縛。
 * name と type_expr は NUL 終端の UTF-8、value は同じ Rin で生成した有効な RinValue。
 */
typedef struct RinType {
    uint32_t id;
    uint64_t env_id;
} RinType;

typedef struct RinBinding {
    const char *name;
    const char *type_expr;
    const RinValue *value;
} RinBinding;

typedef struct RinResolvedBinding {
    const char *name;
    RinType ty;
    const RinValue *value;
} RinResolvedBinding;

/* value は Rin が所有する借用ポインタ。同じ Rin に対する次の評価・呼び出し・解放まで有効。
 * rin_load 成功時と失敗時は NULL。ホスト側で解放してはならない。
 */
typedef struct RinEvalResult {
    bool ok;
    const RinValue *value;
} RinEvalResult;

RinLimits rin_limits_default(void);
RinMemoryConfig rin_memory_config_default(void);
Rin *rin_new(RinLimits limits, RinAllocator alloc);
Rin *rin_new_with_config(RinLimits limits, RinMemoryConfig memory, RinAllocator alloc);
void rin_free(Rin *rin);
bool rin_freeze(Rin *rin);
bool rin_set_math_fma(Rin *rin, RinMathFma mode);
RinMathFma rin_math_fma(const Rin *rin);
bool rin_resolve_type(Rin *rin, const char *type_expr, RinType *out);
bool rin_bind(Rin *rin, const char *name, const char *type_expr, const RinValue *val);
bool rin_bind_resolved(Rin *rin, const char *name, RinType ty, const RinValue *val);
RinEvalResult rin_build(Rin *rin, const char *source);
RinEvalResult rin_eval(Rin *rin, const char *source);
/* bindings は binding_count 個の有効な RinBinding を指すこと（binding_count == 0 なら NULL 可）。
 * binding_count > 0 で bindings == NULL、要素の name/value が NULL、または name が不正 UTF-8 なら RIN_ERR_HOST。
 */
RinEvalResult rin_eval_with(Rin *rin, const char *source, const RinBinding *bindings, size_t binding_count);
RinEvalResult rin_eval_with_resolved(Rin *rin, const char *source, const RinResolvedBinding *bindings, size_t binding_count);
RinEvalResult rin_load(Rin *rin, const char *mod_name, const char *source);
RinEvalResult rin_call(Rin *rin, const char *fn_name, const RinValue *const *args, size_t argc);
RinEvalResult rin_call_in(Rin *rin, const char *mod_name, const char *fn_name, const RinValue *const *args, size_t argc);
bool rin_fn_arity(const Rin *rin, const char *fn_name, size_t *out_arity);
bool rin_fn_arity_in(const Rin *rin, const char *mod_name, const char *fn_name, size_t *out_arity);
size_t rin_fn_sig_format(const Rin *rin, const char *fn_name, char *out_buf, size_t out_cap);
size_t rin_fn_sig_format_in(const Rin *rin, const char *mod_name, const char *fn_name, char *out_buf, size_t out_cap);
/* 直近の対象操作のエラー。エラーなし、または rin == NULL なら NULL。
 * ポインタと文字列は同じ rin の次の評価・呼び出し・状態変更操作（rin_build / rin_eval /
 * rin_eval_with / rin_load / rin_call / rin_call_in / rin_bind / rin_freeze /
 * rin_set_math_fma）、値構築失敗（rin_int 等 / rin_struct_set）、
 * 所有ハンドルを返す読み取り 5 関数（rin_car / rin_cdr / rin_field / rin_unwrap /
 * rin_tuple_get）のハンドル確保失敗、または rin_free まで有効。
 * 状態変更・評価操作は冒頭で前回のエラーをクリアし、失敗時に新しいエラーを記録する。
 * 値構築関数（rin_int 等）および rin_struct_set は失敗時（OOM・NULL 引数・不正 UTF-8 等）に
 * NULL / false を返してエラーを記録する。
 * rin_last_error は無効化しない。
 * NULL rin には記録できない。それ以外の引数エラーは RIN_ERR_HOST、位置は 0。
 * 同じ rin の操作・解放と並行して参照してはならない。
 */
const RinError *rin_last_error(const Rin *rin);

/* 値ハンドル構築・解放（すべて Rin のアロケータ上で確保される。使い終わったら rin_drop で返却）。
 * 確保失敗・NULL 引数・不正 UTF-8 等のエラー時は NULL（rin_struct_set は false）を返し、
 * rin_last_error を更新する。
 * 注意（以下はホスト側の誤用であり、してはならない。rin_struct_set の場合を除き rin_last_error には記録されない）:
 * - RinEvalResult.value で返された借用ポインタを rin_drop や rin_struct_set の第 2 引数に渡してはならない
 *   （rin_drop は無視、rin_struct_set は RIN_ERR_HOST を記録して false を返す）。
 * - 同じハンドルの連続二重 rin_drop は無視されるが、スロット再利用後の古いポインタを渡すと、rin_drop では
 *   再利用先のハンドルを解放し、rin_struct_set では再利用先のハンドルが持つ構造体を書き換えてしまう。
 *   未再利用の解放済みハンドルは rin_struct_set で RIN_ERR_HOST を記録して false を返し、読み取り・入力では
 *   Void や別値を読むことがある。
 * - 別の生存中 Rin で生成したハンドルを rin_drop に渡した場合は無視され、rin_struct_set の第 2 引数に
 *   渡した場合は RIN_ERR_HOST を記録して false を返す。読み取り・入力に渡すと、別値を読むか境界検査の
 *   panic でプロセスが中断（abort）する。rin_free した Rin のハンドルは無効なポインタであり、どの関数にも渡してはならない。
 */
RinValue *rin_int(Rin *rin, int64_t n);
RinValue *rin_float(Rin *rin, double x);
RinValue *rin_bool(Rin *rin, bool b);
RinValue *rin_text(Rin *rin, const char *s);
RinValue *rin_ident(Rin *rin, const char *s);
RinValue *rin_i64s(Rin *rin, const int64_t *ptr, size_t len);
RinValue *rin_f64s(Rin *rin, const double *ptr, size_t len);
RinValue *rin_nil(Rin *rin);
RinValue *rin_cons(Rin *rin, const RinValue *head, const RinValue *tail);
RinValue *rin_none(Rin *rin);
RinValue *rin_some(Rin *rin, const RinValue *val);
RinValue *rin_struct(Rin *rin, const char *name);
bool rin_struct_set(Rin *rin, RinValue *s, const char *field, const RinValue *val);
RinValue *rin_variant(Rin *rin, const char *type_name, const char *variant, const RinValue *data);
void rin_drop(Rin *rin, RinValue *val);

/* 値読み取り（rin_car / rin_cdr / rin_field / rin_unwrap / rin_tuple_get の返り値は rin_drop で返却）。
 * 上記 5 関数でハンドル領域の確保に失敗した場合は NULL を返し、rin_last_error を更新する。
 * rin_as_text / rin_variant_name が返すポインタは NUL 終端ではなく、長さは *out_len で受け取る。
 * 空文字列は非 NULL ポインタと長さ 0 を返す。out_len == NULL や型不一致時は NULL を返し *out_len を書き換えない。
 */
RinTag rin_tag(const Rin *rin, const RinValue *val);
bool rin_as_int(const Rin *rin, const RinValue *val, int64_t *out);
bool rin_as_float(const Rin *rin, const RinValue *val, double *out);
bool rin_as_bool(const Rin *rin, const RinValue *val, bool *out);
const uint8_t *rin_as_text(const Rin *rin, const RinValue *val, size_t *out_len);
const uint8_t *rin_as_ident(const Rin *rin, const RinValue *val, size_t *out_len);
size_t rin_len(const Rin *rin, const RinValue *val);
RinValue *rin_car(Rin *rin, const RinValue *val);
RinValue *rin_cdr(Rin *rin, const RinValue *val);
RinValue *rin_field(Rin *rin, const RinValue *val, const char *field);
const uint8_t *rin_variant_name(const Rin *rin, const RinValue *val, size_t *out_len);
RinValue *rin_unwrap(Rin *rin, const RinValue *val);
RinValue *rin_tuple_get(Rin *rin, const RinValue *val, size_t index);
size_t rin_copy_i64s(const Rin *rin, const RinValue *val, int64_t *out_ptr, size_t out_cap);
size_t rin_copy_f64s(const Rin *rin, const RinValue *val, double *out_ptr, size_t out_cap);

/* スキーマ検証（Cargo フィーチャ std-value が有効なビルドでのみ提供される。
 * --no-default-features でビルドした librin.a には含まれない）。
 */
RinSchema *rin_schema(const char *src);
void rin_schema_free(RinSchema *schema);
bool rin_check(const Rin *rin, const RinValue *val, const RinSchema *schema);
bool rin_check_str(const Rin *rin, const RinValue *val, const char *schema_src);

#ifdef __cplusplus
}
#endif

#endif
