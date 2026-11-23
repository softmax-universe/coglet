#ifndef C_AUDIO_RING_H
#define C_AUDIO_RING_H
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
typedef struct FARing FARing;
FARing *fa_create(size_t capacity);
void fa_destroy(FARing *r);
bool fa_write(FARing *r, const int16_t *samples, size_t count);
size_t fa_read(FARing *r, int16_t *samples, size_t count);
// Call on producer/control thread, never concurrently with fa_write.
void fa_flush(FARing *r);
uint64_t fa_generation(FARing *r);
size_t fa_render(FARing *r, float *samples, size_t count, float gain);
// Split entry seam to exercise a flush between epoch and cursor snapshots.
size_t fa_render_generation(FARing *r, float *samples, size_t count, float gain, uint64_t generation);
uint64_t fa_played(FARing *r);
float fa_peak(FARing *r);
void fa_gain(FARing *r, float gain);
void fa_measure(FARing *r, const int16_t *samples, size_t count);
void fa_fail(FARing *r);
bool fa_failed(FARing *r);
#endif
