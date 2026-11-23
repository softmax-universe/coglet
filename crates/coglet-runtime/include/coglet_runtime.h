#ifndef COGLET_RUNTIME_H
#define COGLET_RUNTIME_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* Trusted application configuration: UTF-8 JSON with surface and
 * permitted_coglets fields. Immutable per process; call before surface_open.
 * No authority comes from hello. This configuration API is runtime-owned,
 * not a new surface wire message. Returns 0 or -1. */
int32_t coglet_runtime_configure(const uint8_t *config, size_t n);
typedef struct coglet_surface_link coglet_surface_link;
int32_t coglet_surface_open(uint32_t abi_version, coglet_surface_link **out);
int32_t coglet_surface_send(coglet_surface_link *, const uint8_t *cbor, size_t n);
int32_t coglet_surface_recv(coglet_surface_link *, uint8_t *out, size_t cap, size_t *n);
void coglet_surface_close(coglet_surface_link *);
/* All operations serialized by caller on a non-audio control queue.
 * 0 success; 1 would-block; 2 short output buffer (no dequeue);
 * -1 invalid argument; -2 closed/fatal. recv sets n=0 on would-block.
 * Buffers caller-owned, never retained. No calls after close. */
#ifdef __cplusplus
}
#endif
#endif
