#include "wasm_export.h"
#include "wasm_exec_env.h"
#include "wasm_runtime.h"
#include <stdint.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>

typedef struct {
    wasm_module_inst_t instance;
    wasm_exec_env_t env;
} Context;
/* All owner access is serialized by the Rust runtime guard. Instances and
 * execution environments are NEVER cached. Loader backing stays writable and
 * alive until unload; comparisons use a separate, unmodified original. */
static struct {
    uint64_t id;
    uint8_t *backing, *original;
    void *pool;
    wasm_module_t module;
    uint32_t len, runtime_bytes;
    int initialized, allow_db;
} owner;
static uint64_t loads;
static uint32_t failure_stage;
uint64_t ce_owner_id(void) { return owner.id; }
uint64_t ce_owner_loads(void) { return loads; }
uint32_t ce_owner_failure_stage(void) { return failure_stage; }
void ce_release_owner(uint64_t id) {
    if (owner.id != id) return;
    if (owner.module) wasm_runtime_unload(owner.module);
    if (owner.initialized) wasm_runtime_destroy();
    free(owner.pool);
    free(owner.backing);
    free(owner.original);
    memset(&owner, 0, sizeof(owner));
}

extern uint32_t ce_host_call(void *state, const char *module, const char *name,
                             uint64_t *args, uint32_t argc);
static void coglet_call(wasm_exec_env_t env, uint64_t *args) {
    wasm_module_inst_t inst = wasm_runtime_get_module_inst(env);
    void *state = wasm_runtime_get_custom_data(inst);
    const char *name = wasm_runtime_get_function_attachment(env);
    if (!state) { wasm_runtime_set_exception(inst, "missing coglet host"); return; }
    args[0] = ce_host_call(state, "coglet_v0", name, args, 2);
}
static void coglet_db_call(wasm_exec_env_t env, uint64_t *args);
static NativeSymbol coglet_symbols[] = {
    {"timer", coglet_call, "(ii)i", "timer"},
    {"emit", coglet_call, "(ii)i", "emit"},
    {"llm", coglet_call, "(ii)i", "llm"},
    {"log", coglet_call, "(ii)i", "log"},
};
static NativeSymbol coglet_db_symbols[] = {
    {"db", coglet_db_call, "(iiii)i", "db"},
};
static void coglet_db_call(wasm_exec_env_t env, uint64_t *args) {
    wasm_module_inst_t inst = wasm_runtime_get_module_inst(env);
    void *state = wasm_runtime_get_custom_data(inst);
    if (!state) { wasm_runtime_set_exception(inst, "missing coglet host"); return; }
    args[0] = ce_host_call(state, "coglet_v0", "db", args, 4);
}
typedef struct { const char *name; uint32_t argc; } WasiImport;
#define IMPORT(name, sig, argc) static WasiImport wasi_##name = {#name, argc};
#include "wasi.def"
#undef IMPORT
static void wasi_call(wasm_exec_env_t env, uint64_t *args) {
    wasm_module_inst_t inst = wasm_runtime_get_module_inst(env);
    void *state = wasm_runtime_get_custom_data(inst);
    WasiImport *import = wasm_runtime_get_function_attachment(env);
    if (!state) { wasm_runtime_set_exception(inst, "missing coglet host"); return; }
    args[0] = ce_host_call(state, "wasi_snapshot_preview1", import->name, args, import->argc);
}
static NativeSymbol wasi_symbols[] = {
#define IMPORT(name, sig, argc) {#name, wasi_call, sig, &wasi_##name},
#include "wasi.def"
#undef IMPORT
};
void ce_set_host(Context *c, void *state) {
    wasm_runtime_set_custom_data(c->instance, state);
}
void ce_host_abort(Context *c) { wasm_runtime_set_exception(c->instance, "coglet host aborted"); }

void ce_destroy(Context *c) {
    if (!c) return;
    if (c->env) wasm_runtime_destroy_exec_env(c->env);
    if (c->instance) wasm_runtime_deinstantiate(c->instance);
    free(c);
}
Context *ce_create(uint64_t id, const uint8_t *bytes, uint32_t len, uint32_t runtime_bytes,
                   uint32_t stack_size, uint32_t pages, int allow_db) {
    failure_stage = 1;
    if (!owner.module || owner.id != id || owner.len != len ||
        owner.runtime_bytes != runtime_bytes || owner.allow_db != allow_db ||
        memcmp(owner.original, bytes, len) != 0) {
        /* Evict BEFORE allocating replacement, including on failure. Subtract
         * both byte buffers with checked arithmetic before provisioning pool. */
        ce_release_owner(owner.id);
        if (!id || len > runtime_bytes / 2 || runtime_bytes - 2 * len < 1024)
            return NULL;
        owner.id = id;
        owner.len = len;
        owner.runtime_bytes = runtime_bytes;
        owner.allow_db = allow_db;
        owner.backing = malloc(len);
        owner.original = malloc(len);
        owner.pool = malloc(runtime_bytes - 2 * len);
        if (!owner.backing || !owner.original || !owner.pool) goto fail_owner;
        memcpy(owner.backing, bytes, len);
        memcpy(owner.original, bytes, len);
        RuntimeInitArgs init = {0};
        init.mem_alloc_type = Alloc_With_Pool;
        init.mem_alloc_option.pool.heap_buf = owner.pool;
        init.mem_alloc_option.pool.heap_size = runtime_bytes - 2 * len;
        if (!wasm_runtime_full_init(&init)) goto fail_owner;
        owner.initialized = 1;
        if (!wasm_runtime_register_natives_raw("coglet_v0", coglet_symbols, sizeof(coglet_symbols)/sizeof(coglet_symbols[0]))) goto fail_owner;
        if (allow_db && !wasm_runtime_register_natives_raw("coglet_v0", coglet_db_symbols, sizeof(coglet_db_symbols)/sizeof(coglet_db_symbols[0]))) goto fail_owner;
        if (!wasm_runtime_register_natives_raw("wasi_snapshot_preview1", wasi_symbols, sizeof(wasi_symbols)/sizeof(wasi_symbols[0]))) goto fail_owner;
        wasm_runtime_set_log_level(0);
        char error[256] = {0};
        owner.module = wasm_runtime_load(owner.backing, len, error, sizeof(error));
        if (!owner.module) goto fail_owner;
        if (loads != UINT64_MAX) loads++;
    }
    failure_stage = 2;
    Context *c = calloc(1, sizeof(*c));
    if (!c) goto fail_owner;
    char error[256] = {0};
    struct InstantiationArgs2 *args = NULL;
    if (!wasm_runtime_instantiation_args_create(&args)) goto fail_instance;
    wasm_runtime_instantiation_args_set_default_stack_size(args, stack_size);
    wasm_runtime_instantiation_args_set_max_memory_pages(args, pages);
    wasm_runtime_instantiation_args_set_host_managed_heap_size(args, 0);
    c->instance = wasm_runtime_instantiate_ex2(owner.module, args, error, sizeof(error));
    wasm_runtime_instantiation_args_destroy(args);
    if (!c->instance) goto fail_instance;
    failure_stage = 3;
    c->env = wasm_runtime_create_exec_env(c->instance, stack_size);
    if (!c->env) goto fail_instance;
    failure_stage = 0;
    return c;
fail_instance:
    ce_destroy(c);
fail_owner:
    ce_release_owner(owner.id);
    return NULL;
}
void ce_set_budget(Context *c, int budget) {
    wasm_runtime_set_instruction_count_limit(c->env, budget);
}
int ce_remaining(Context *c) { return c->env->instructions_to_execute; }
int ce_has(Context *c, const char *name) { return wasm_runtime_lookup_function(c->instance, name) != NULL; }
int ce_call(Context *c, const char *name, uint32_t argc, uint32_t *argv) {
    wasm_function_inst_t f = wasm_runtime_lookup_function(c->instance, name);
    if (!f) return -1;
    return wasm_runtime_call_wasm(c->env, f, argc, argv) ? 1 : 0;
}
void ce_terminate(Context *c) { wasm_runtime_terminate(c->instance); }
int ce_valid(Context *c, uint32_t ptr, uint32_t len) { return wasm_runtime_validate_app_addr(c->instance, ptr, len); }
uint64_t ce_memory_size(Context *c) {
    wasm_memory_inst_t mem = wasm_runtime_get_default_memory(c->instance);
    if (!mem) return 0;
    uint64_t pages = wasm_memory_get_cur_page_count(mem);
    uint64_t bps = wasm_memory_get_bytes_per_page(mem);
    if (bps != 0 && pages > UINT64_MAX / bps) return UINT64_MAX;
    return pages * bps;
}
// Copy under WAMR's exception lock: terminate may overwrite its internal
// buffer concurrently. Never expose a borrowed pointer to that mutable buffer.
int ce_copy_exception(Context *c, char *output, uint32_t capacity) {
    if (!output || capacity < EXCEPTION_BUF_LEN) return -1;
    return wasm_copy_exception((WASMModuleInstance *)c->instance, output) ? 1 : 0;
}
int ce_write(Context *c, uint32_t ptr, const uint8_t *bytes, uint32_t len) {
    if (!wasm_runtime_validate_app_addr(c->instance, ptr, len)) return 0;
    if (len) memcpy(wasm_runtime_addr_app_to_native(c->instance, ptr), bytes, len);
    return 1;
}
int ce_read(Context *c, uint32_t ptr, uint8_t *bytes, uint32_t len) {
    if (!wasm_runtime_validate_app_addr(c->instance, ptr, len)) return 0;
    if (len) memcpy(bytes, wasm_runtime_addr_app_to_native(c->instance, ptr), len);
    return 1;
}
