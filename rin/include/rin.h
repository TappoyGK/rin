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
    RIN_TAG_STR = 5,
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
} RinTag;

typedef struct RinLimits {
    size_t steps;
    size_t depth;
    size_t memory_bytes;
} RinLimits;

typedef struct RinAllocator {
    void *ctx;
    uint8_t *(*alloc)(void *ctx, size_t size, size_t align);
    void (*dealloc)(void *ctx, uint8_t *ptr, size_t size, size_t align);
} RinAllocator;

typedef struct RinMachine RinMachine;
typedef struct RinValue RinValue;
typedef struct RinSchema RinSchema;

typedef struct RinEvalResult {
    bool ok;
    RinValue *value;
    char *error_message;
    size_t error_line;
    size_t error_col;
} RinEvalResult;

RinLimits rin_limits_default(void);
RinMachine *rin_new(RinLimits limits, RinAllocator alloc);
void rin_free(RinMachine *machine);
bool rin_bind(RinMachine *machine, const char *name, const RinValue *val);
RinEvalResult rin_eval(RinMachine *machine, const char *source);
RinEvalResult rin_load(RinMachine *machine, const char *mod_name, const char *source);
RinEvalResult rin_call(RinMachine *machine, const char *fn_name, const RinValue *const *args, size_t argc);
RinEvalResult rin_call_in(RinMachine *machine, const char *mod_name, const char *fn_name, const RinValue *const *args, size_t argc);
const char *rin_last_backtrace(const RinMachine *machine);
void rin_res_free(RinEvalResult *res);

RinValue *rin_int(int64_t n);
RinValue *rin_float(double x);
RinValue *rin_bool(bool b);
RinValue *rin_str(const char *s);
RinValue *rin_i64s(const int64_t *ptr, size_t len);
RinValue *rin_f64s(const double *ptr, size_t len);
RinValue *rin_nil(void);
RinValue *rin_cons(const RinValue *head, const RinValue *tail);
RinValue *rin_none(void);
RinValue *rin_some(const RinValue *val);
RinValue *rin_struct(const char *name);
bool rin_struct_set(RinValue *s, const char *field, const RinValue *val);
RinValue *rin_variant(const char *type_name, const char *variant, const RinValue *data);
void rin_drop(RinValue *val);

RinTag rin_tag(const RinValue *val);
bool rin_as_int(const RinValue *val, int64_t *out);
bool rin_as_float(const RinValue *val, double *out);
bool rin_as_bool(const RinValue *val, bool *out);
const uint8_t *rin_as_str(const RinValue *val, size_t *out_len);
size_t rin_len(const RinValue *val);
RinValue *rin_car(const RinValue *val);
RinValue *rin_cdr(const RinValue *val);
RinValue *rin_field(const RinValue *val, const char *field);
const uint8_t *rin_variant_name(const RinValue *val, size_t *out_len);
RinValue *rin_unwrap(const RinValue *val);
RinValue *rin_tuple_get(const RinValue *val, size_t index);
size_t rin_copy_i64s(const RinValue *val, int64_t *out_ptr, size_t out_cap);
size_t rin_copy_f64s(const RinValue *val, double *out_ptr, size_t out_cap);

RinSchema *rin_schema(const char *src);
void rin_schema_free(RinSchema *schema);
bool rin_check(const RinValue *val, const RinSchema *schema);
bool rin_check_str(const RinValue *val, const char *schema_src);

#ifdef __cplusplus
}
#endif

#endif
