// Ring/discard/generation design adapted from Polyworld experiments/voice (MIT).
#include "CAudioRing.h"
#include <stdatomic.h>
#include <stdlib.h>
#include <math.h>
#include <string.h>
_Static_assert(ATOMIC_LLONG_LOCK_FREE == 2, "Realtime counters must be lock free");
_Static_assert(ATOMIC_SHORT_LOCK_FREE == 2, "Realtime samples must be lock free");
struct FARing {
    size_t capacity;
    _Atomic(int16_t) *samples;
    atomic_ullong read, write, discard, generation, played;
    atomic_uint peak, gain;
    atomic_bool failed;
};
FARing *fa_create(size_t capacity) {
    if (!capacity) return NULL;
    FARing *r = calloc(1, sizeof(*r));
    if (!r) return NULL;
    r->samples = calloc(capacity, sizeof(*r->samples));
    if (!r->samples) { free(r); return NULL; }
    r->capacity = capacity;
    atomic_init(&r->read, 0); atomic_init(&r->write, 0);
    atomic_init(&r->discard, 0); atomic_init(&r->generation, 0);
    atomic_init(&r->played, 0); atomic_init(&r->peak, 0);
    atomic_init(&r->gain, 32768); atomic_init(&r->failed, false);
    for (size_t i = 0; i < capacity; i++) atomic_init(&r->samples[i], 0);
    return r;
}
void fa_destroy(FARing *r) { if (r) { free(r->samples); free(r); } }
static uint64_t position(FARing *r) {
    uint64_t a = atomic_load_explicit(&r->read, memory_order_acquire);
    uint64_t b = atomic_load_explicit(&r->discard, memory_order_acquire);
    return a > b ? a : b;
}
bool fa_write(FARing *r, const int16_t *samples, size_t count) {
    uint64_t w = atomic_load_explicit(&r->write, memory_order_relaxed);
    if (count > r->capacity - (w - position(r))) return false;
    for (size_t i = 0; i < count; i++)
        atomic_store_explicit(&r->samples[(w+i)%r->capacity], samples[i], memory_order_relaxed);
    atomic_store_explicit(&r->write, w+count, memory_order_release);
    return true;
}
size_t fa_read(FARing *r, int16_t *samples, size_t count) {
    uint64_t p = position(r);
    uint64_t w = atomic_load_explicit(&r->write, memory_order_acquire);
    if (count > w-p) count = w-p;
    for (size_t i = 0; i < count; i++)
        samples[i] = atomic_load_explicit(&r->samples[(p+i)%r->capacity], memory_order_relaxed);
    atomic_store_explicit(&r->read, p+count, memory_order_release);
    return count;
}
void fa_flush(FARing *r) {
    atomic_fetch_add(&r->generation, 1);
    atomic_store(&r->discard, atomic_load(&r->write));
    atomic_fetch_add(&r->generation, 1);
    atomic_store(&r->peak, 0);
}
uint64_t fa_generation(FARing *r) { return atomic_load(&r->generation); }
size_t fa_render(FARing *r, float *samples, size_t count, float gain) {
    return fa_render_generation(r, samples, count, gain, fa_generation(r));
}
size_t fa_render_generation(FARing *r, float *samples, size_t count, float gain, uint64_t g) {
    gain *= atomic_load(&r->gain) / 32768.f;
    uint64_t p = position(r);
    uint64_t w = atomic_load_explicit(&r->write, memory_order_acquire);
    // Validate the epoch AFTER cursor snapshots, BEFORE committing consumption.
    // A later flush can only discard through a write >= w. Thus p+n cannot
    // advance past that boundary and eat a newly enqueued utterance.
    if ((g & 1) || g != fa_generation(r)) {
        memset(samples, 0, count*sizeof(float)); return 0;
    }
    size_t n = count < w-p ? count : w-p;
    float peak = 0;
    for (size_t i = 0; i < n; i++) {
        samples[i] = atomic_load_explicit(&r->samples[(p+i)%r->capacity], memory_order_relaxed) / 32768.f * gain;
        peak = fmaxf(peak, fabsf(samples[i]));
    }
    for (size_t i = n; i < count; i++) samples[i] = 0;
    atomic_store_explicit(&r->read, p+n, memory_order_release);
    if ((g & 1) || g != fa_generation(r)) {
        memset(samples, 0, count*sizeof(float)); n = 0; peak = 0;
    }
    atomic_fetch_add(&r->played, n);
    atomic_store(&r->peak, (unsigned)(peak * 32768));
    return n;
}
uint64_t fa_played(FARing *r) { return atomic_load(&r->played); }
float fa_peak(FARing *r) { return atomic_load(&r->peak) / 32768.f; }
void fa_gain(FARing *r, float gain) { atomic_store(&r->gain, (unsigned)(fminf(1, fmaxf(0, gain))*32768)); }
void fa_measure(FARing *r, const int16_t *samples, size_t count) {
    unsigned peak = 0;
    for (size_t i = 0; i < count; i++) {
        unsigned value = abs((int)samples[i]);
        if (value > peak) peak = value;
    }
    atomic_store(&r->peak, peak);
}
void fa_fail(FARing *r) { atomic_store(&r->failed, true); }
bool fa_failed(FARing *r) { return atomic_load(&r->failed); }
