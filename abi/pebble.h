#ifndef TOBBINS_PEBBLE_ABI_H
#define TOBBINS_PEBBLE_ABI_H
#include <stdint.h>
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif
#define TOBBINS_PEBBLE_ABI 0x00010001u
#define TOB_NATIVE_OBJECT(type_id) (5u | ((uint32_t)(type_id) << 16))
enum tob_pebble_kind { TOB_VOID=0, TOB_U32=1, TOB_F64=2, TOB_UTF8=3,
    TOB_BUFFER=4, TOB_OBJECT=5, TOB_HANDLE=6, TOB_TREE=7 };
typedef void (*tob_pebble_release)(void *owner);
typedef struct tob_pebble_value {
    uint32_t tag, reserved; /* reserved is the package-local type ID for OBJECT */
    uint64_t scalar;
    const uint8_t *data;
    size_t len;
    void *owner;
    tob_pebble_release release;
    const uint64_t *references;
    size_t references_len;
} tob_pebble_value;
/* 0: success, 1: recoverable Exception, 2: fatal Error. Never unwind. */
typedef uint32_t (*tob_pebble_call)(const tob_pebble_value *, size_t, tob_pebble_value *);
typedef struct tob_pebble_export {
    const uint8_t *name;
    size_t name_len;
    const uint32_t *parameters;
    size_t parameters_len;
    uint32_t result, reserved;
    tob_pebble_call call;
} tob_pebble_export;
typedef struct tob_pebble_descriptor {
    uint32_t abi, size;
    const uint8_t *name;
    size_t name_len;
    const uint8_t *version;
    size_t version_len;
    const tob_pebble_export *exports;
    size_t exports_len;
} tob_pebble_descriptor;
typedef const tob_pebble_descriptor *(*tob_pebble_entry)(void);
#ifdef __cplusplus
}
#endif
#endif
