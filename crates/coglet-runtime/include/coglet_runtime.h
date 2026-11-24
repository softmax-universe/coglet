#ifndef COGLET_RUNTIME_H
#define COGLET_RUNTIME_H

#include <stdarg.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <stdint.h>
#include <stddef.h>
typedef struct coglet_surface_link coglet_surface_link;

#define SURFACE_MAX_FRAME 1048576

#define SURFACE_QUEUE 128

/**
 * Supervisor status: 0 running, 1 stopped, -2 new guest failure or caught panic.
 * Historical failed batches are never replayed by Restart.
 */
int32_t coglet_runtime_status(void);

/**
 * Start/recover the durable coordinator from trusted JSON {wal, setup, native_schedule?}.
 * A caught panic requires closing every link and calling stop before restart.
 * # Safety
 * `config` references `n` readable immutable bytes for this call.
 */
int32_t coglet_runtime_start(const uint8_t *config, size_t n);

/**
 * Stop only after all links close. Returns 1 while links hold the runtime.
 * Poisoned service state is discarded, NEVER unpoisoned for continued execution.
 */
int32_t coglet_runtime_stop(void);

/**
 * Configure the process's immutable supervisor binding from UTF-8 JSON.
 * Repeating identical configuration is idempotent; replacing identity requires
 * a new runtime process. This is NOT a surface wire message.
 * # Safety
 * `config` must reference `n` readable bytes for this call, with no concurrent mutation.
 */
int32_t coglet_runtime_configure(const uint8_t *config, size_t n);

/**
 * Open an unnegotiated link in the already-configured runtime.
 * # Safety
 * `out` must be null or point to writable, aligned storage for one handle pointer.
 */
int32_t coglet_surface_open(uint32_t abi_version, coglet_surface_link **out);

/**
 * Copy and validate one deterministic CBOR message. No caller pointer is retained.
 * # Safety
 * `link` must be a live handle, exclusively accessed for the call; `cbor` must
 * reference `n` readable bytes without concurrent mutation. No calls after close.
 */
int32_t coglet_surface_send(coglet_surface_link *link, const uint8_t *cbor, size_t n);

/**
 * Poll one response; short buffers and size queries do not dequeue it.
 * # Safety
 * `link` must be live and exclusively accessed. `n` points to writable usize.
 * `out` points to `cap` writable bytes, or is null with cap=0 for size query.
 * Output buffers must not alias the handle or each other.
 */
int32_t coglet_surface_recv(coglet_surface_link *link, uint8_t *out, size_t cap, size_t *n);

/**
 * Apply disconnect and free the handle. Null is a no-op. Fenced links are
 * discarded without calling into uncertain service state.
 * # Safety
 * Non-null `link` must be a live handle from open, with no concurrent access.
 * The handle must never be used again, including a second close.
 */
void coglet_surface_close(coglet_surface_link *link);

#if defined(COGLET_RUNTIME_TEST_HOOKS)
/**
 * TEST BUILDS ONLY: inject an unwind while holding both runtime ownership and
 * service locks, exercising the poisoned-state path through an actual C entry.
 * Not present in ordinary release artifacts; never exposed as guest authority.
 */
int32_t coglet_runtime_test_panic(void);
#endif

#endif  /* COGLET_RUNTIME_H */
