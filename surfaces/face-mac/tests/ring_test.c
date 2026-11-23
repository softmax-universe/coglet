#include "CAudioRing.h"
#include <assert.h>
#include <pthread.h>
#include <stdio.h>
static void *produce(void *arg) {
    FARing *r = arg;
    for (int i = 0; i < 100000; i++) {
        int16_t value = i % 30000;
        while (!fa_write(r, &value, 1)) {}
    }
    return NULL;
}
int main(void) {
    FARing *r = fa_create(4);
    int16_t a[] = {1, 2, 3, 4}, b[4];
    assert(fa_write(r, a, 3)); assert(!fa_write(r, a, 2));
    assert(fa_read(r, b, 2) == 2 && b[0] == 1 && b[1] == 2);
    assert(fa_write(r, a, 3));
    assert(fa_read(r, b, 4) == 4 && b[0] == 3 && b[3] == 3);
    assert(fa_write(r, a, 4));
    uint64_t generation = fa_generation(r);
    fa_flush(r); assert(fa_generation(r) != generation);
    float out[4];
    assert(fa_render(r, out, 4, 1) == 0);
    for (int i = 0; i < 4; i++) assert(out[i] == 0);
    assert(fa_played(r) == 0);
    assert(fa_write(r, a, 2)); assert(fa_render(r, out, 4, 1) == 2);
    assert(fa_played(r) == 2 && out[2] == 0);
    fa_destroy(r);
    r = fa_create(128);
    pthread_t thread; assert(pthread_create(&thread, NULL, produce, r) == 0);
    for (int i = 0; i < 100000; i++) {
        int16_t value;
        while (!fa_read(r, &value, 1)) {}
        assert(value == i % 30000);
    }
    assert(pthread_join(thread, NULL) == 0);
    fa_destroy(r);
    puts("ring: wrap/overflow/discard/generation/render/position/concurrent SPSC passed");
}
