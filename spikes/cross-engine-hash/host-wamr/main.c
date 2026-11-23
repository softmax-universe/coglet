// WAMR side of the cross-engine hash spike (interpreter and AOT, chosen by the file given).
//   xhash-wamr <module.wasm|module.aot> [steps]
// Prints one line per step: "<step> <guest_hash> <host_hash_of_state_blob>" (hex), same as the
// Wasmtime host. gen_step must stay identical to host-wasmtime/src/main.rs.
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "wasm_export.h"

static uint64_t splitmix(uint64_t *s) {
    *s += 0x9E3779B97F4A7C15ULL;
    uint64_t z = *s;
    z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9ULL;
    z = (z ^ (z >> 27)) * 0x94D049BB133111EBULL;
    return z ^ (z >> 31);
}

static uint64_t fnv1a(const uint8_t *p, size_t n) {
    uint64_t h = 0xcbf29ce484222325ULL;
    for (size_t i = 0; i < n; i++) { h ^= p[i]; h *= 0x100000001b3ULL; }
    return h;
}

static uint32_t f32bits(float f) { uint32_t u; memcpy(&u, &f, 4); return u; }
static uint64_t f64bits(double f) { uint64_t u; memcpy(&u, &f, 8); return u; }

static size_t gen_step(uint32_t step, uint8_t *buf, uint64_t *logical_time, uint64_t *seed) {
    uint64_t s = (uint64_t)step * 0xD1B54A32D192ED03ULL + 12345ULL;
    size_t n = splitmix(&s) % 17;
    *logical_time = (uint64_t)step * 16666667ULL + splitmix(&s) % 1000;
    *seed = splitmix(&s);
    for (size_t i = 0; i < n; i++) {
        uint8_t kind = splitmix(&s) % 12;
        uint64_t ra = splitmix(&s);
        uint32_t a;
        switch (ra % 16) {
        case 10: a = 0x7fc00000u | ((uint32_t)(ra >> 8) & 0x003fffffu) | (((uint32_t)(ra >> 40) & 1u) << 31); break;
        case 11: a = 0x7f800000u + (1u + ((uint32_t)(ra >> 8) & 0x003ffffeu)); break;
        case 12: a = 0x7f800000u; break;
        case 13: a = 0xff800000u; break;
        case 14: a = 0x80000000u; break;
        case 15: a = (uint32_t)(ra >> 8) & 0x007fffffu; break;
        default: a = f32bits((((float)(uint32_t)(ra >> 40)) / 16777216.0f - 0.5f) * 8.0f);
        }
        uint64_t rb = splitmix(&s);
        uint64_t b;
        switch (rb % 16) {
        case 10: b = 0x7ff8000000000000ULL | ((rb >> 12) & 0x0007ffffffffffffULL) | ((rb >> 63) << 63); break;
        case 11: b = 0x7ff0000000000000ULL + (1ULL + ((rb >> 12) & 0x0007fffffffffffeULL)); break;
        case 12: b = 0x7ff0000000000000ULL; break;
        case 13: b = 0xfff0000000000000ULL; break;
        case 14: b = 0x8000000000000000ULL; break;
        case 15: b = (rb >> 12) & 0x000fffffffffffffULL; break;
        default: b = f64bits((((double)(uint64_t)(rb >> 11)) / 9007199254740992.0 - 0.5) * 8.0);
        }
        uint8_t *r = buf + i * 16;
        memset(r, 0, 16);
        r[0] = kind;
        memcpy(r + 4, &a, 4);
        memcpy(r + 8, &b, 8);
    }
    return n * 16;
}

static wasm_function_inst_t must_lookup(wasm_module_inst_t inst, const char *name) {
    wasm_function_inst_t f = wasm_runtime_lookup_function(inst, name);
    if (!f) { fprintf(stderr, "missing export %s\n", name); exit(2); }
    return f;
}

int main(int argc, char **argv) {
    if (argc < 2) { fprintf(stderr, "usage: xhash-wamr <module> [steps]\n"); return 2; }
    uint32_t steps = argc > 2 ? (uint32_t)atoi(argv[2]) : 1000;

    FILE *fp = fopen(argv[1], "rb");
    if (!fp) { perror("open"); return 2; }
    fseek(fp, 0, SEEK_END);
    long sz = ftell(fp);
    fseek(fp, 0, SEEK_SET);
    uint8_t *img = malloc(sz);
    if (fread(img, 1, sz, fp) != (size_t)sz) return 2;
    fclose(fp);

    char err[256] = {0};
    if (!wasm_runtime_init()) { fprintf(stderr, "runtime init failed\n"); return 2; }
    wasm_module_t mod = wasm_runtime_load(img, (uint32_t)sz, err, sizeof err);
    if (!mod) { fprintf(stderr, "load failed: %s\n", err); return 3; }
    wasm_runtime_set_wasi_args(mod, NULL, 0, NULL, 0, NULL, 0, NULL, 0);
    wasm_module_inst_t inst = wasm_runtime_instantiate(mod, 256 * 1024, 0, err, sizeof err);
    if (!inst) { fprintf(stderr, "instantiate failed: %s\n", err); return 3; }
    wasm_exec_env_t env = wasm_runtime_create_exec_env(inst, 256 * 1024);

    wasm_function_inst_t f_alloc = must_lookup(inst, "alloc_events");
    wasm_function_inst_t f_handle = must_lookup(inst, "handle");
    wasm_function_inst_t f_sptr = must_lookup(inst, "state_ptr");
    wasm_function_inst_t f_slen = must_lookup(inst, "state_len");

    uint8_t evbuf[4096];
    for (uint32_t step = 0; step < steps; step++) {
        uint64_t t, seed;
        size_t len = gen_step(step, evbuf, &t, &seed);
        wasm_val_t a1[1] = {{.kind = WASM_I32, .of.i32 = (int32_t)len}};
        wasm_val_t r1[1] = {{.kind = WASM_I32}};
        if (!wasm_runtime_call_wasm_a(env, f_alloc, 1, r1, 1, a1)) goto fail;
        uint32_t p = (uint32_t)r1[0].of.i32;
        void *native = wasm_runtime_addr_app_to_native(inst, p);
        if (!native && len) { fprintf(stderr, "bad events pointer\n"); return 3; }
        memcpy(native, evbuf, len);
        wasm_val_t a[5] = {
            {.kind = WASM_I32, .of.i32 = (int32_t)p},   {.kind = WASM_I32, .of.i32 = (int32_t)len},
            {.kind = WASM_I32, .of.i32 = (int32_t)step}, {.kind = WASM_I64, .of.i64 = (int64_t)t},
            {.kind = WASM_I64, .of.i64 = (int64_t)seed}};
        wasm_val_t r[1] = {{.kind = WASM_I64}};
        if (!wasm_runtime_call_wasm_a(env, f_handle, 1, r, 5, a)) goto fail;
        wasm_val_t rs[1] = {{.kind = WASM_I32}}, rl[1] = {{.kind = WASM_I32}};
        if (!wasm_runtime_call_wasm_a(env, f_sptr, 1, rs, 0, NULL)) goto fail;
        if (!wasm_runtime_call_wasm_a(env, f_slen, 1, rl, 0, NULL)) goto fail;
        const uint8_t *st = wasm_runtime_addr_app_to_native(inst, (uint32_t)rs[0].of.i32);
        printf("%u %016llx %016llx", step, (unsigned long long)r[0].of.i64,
               (unsigned long long)fnv1a(st, (uint32_t)rl[0].of.i32));
        if (getenv("XHASH_STATE")) { printf(" "); for (uint32_t k = 0; k < (uint32_t)rl[0].of.i32; k++) printf("%02x", st[k]); }
        printf("\n");
    }
    wasm_runtime_destroy_exec_env(env);
    wasm_runtime_deinstantiate(inst);
    wasm_runtime_unload(mod);
    wasm_runtime_destroy();
    return 0;
fail:
    fprintf(stderr, "trap: %s\n", wasm_runtime_get_exception(inst));
    return 4;
}
