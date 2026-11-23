#include "wasm_export.h"
#include <stdio.h>
#include <stddef.h>
/* Fixed aligned allocator budget, not total process RSS. */
static union { uint64_t align; unsigned char bytes[2 * 1024 * 1024]; } pool;
int probe_init(void) {
    RuntimeInitArgs args = {0};
    args.mem_alloc_type = Alloc_With_Pool;
    args.mem_alloc_option.pool.heap_buf = pool.bytes;
    args.mem_alloc_option.pool.heap_size = sizeof(pool.bytes);
    return wasm_runtime_full_init(&args);
}
void probe_memory(void) {
    mem_alloc_info_t info = {0};
    if (!wasm_runtime_get_mem_alloc_info(&info)) return;
    printf("pool_total_bytes=%u pool_used_bytes=%u pool_peak_bytes=%u\n",
           info.total_size, info.total_size - info.total_free_size, info.highmark_size);
    fflush(stdout);
}
