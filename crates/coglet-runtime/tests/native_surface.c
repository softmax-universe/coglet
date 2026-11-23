/* Native C caller: ownership/buffer probes against the real linked library.
 * Files contain independent wire stimuli, not expected protocol responses. */
#include "coglet_runtime.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
static unsigned char *readfile(const char *path, size_t *n) {
    FILE *f = fopen(path, "rb"); assert(f);
    assert(fseek(f, 0, SEEK_END) == 0); long size = ftell(f); assert(size > 0 && size <= 1048576);
    rewind(f); *n = (size_t)size; unsigned char *buf = malloc(*n); assert(buf);
    assert(fread(buf, 1, *n, f) == *n); fclose(f); return buf;
}
int main(int argc, char **argv) {
    assert(argc == 4);
    coglet_surface_link *link = NULL; size_t n = 7;
    assert(coglet_surface_open(1, &link) == -1 && link == NULL);
    size_t config_n, hello_n, health_n;
    unsigned char *config = readfile(argv[1], &config_n);
    unsigned char *hello = readfile(argv[2], &hello_n);
    unsigned char *health = readfile(argv[3], &health_n);
    assert(coglet_runtime_configure(NULL, config_n) == -1);
    assert(coglet_runtime_configure(config, config_n) == 0);
    assert(coglet_runtime_configure(config, config_n) == 0);
    memset(config, 0, config_n); free(config); /* no retained caller pointers */
    assert(coglet_surface_open(2, &link) == -1 && link == NULL);
    assert(coglet_surface_open(1, NULL) == -1);
    assert(coglet_surface_open(1, &link) == 0 && link);
    assert(coglet_surface_send(link, NULL, hello_n) == -1);
    assert(coglet_surface_send(link, hello, 0) == -1);
    assert(coglet_surface_send(link, hello, 1048577) == -1);
    assert(coglet_surface_recv(link, NULL, 0, &n) == 1 && n == 0);
    assert(coglet_surface_recv(link, NULL, 1, &n) == -1);
    assert(coglet_surface_recv(link, NULL, 0, NULL) == -1);
    assert(coglet_surface_send(link, hello, hello_n) == 0);
    memset(hello, 0, hello_n); free(hello); /* response independent of input lifetime */
    assert(coglet_surface_recv(link, NULL, 0, &n) == 2 && n > 1);
    size_t needed = n; unsigned char *buf = malloc(needed); assert(buf); memset(buf, 0x55, needed);
    assert(coglet_surface_recv(link, buf, needed - 1, &n) == 2 && n == needed);
    for (size_t i = 0; i < needed; ++i) assert(buf[i] == 0x55);
    assert(coglet_surface_recv(link, buf, needed, &n) == 0 && n == needed);
    assert(coglet_surface_recv(link, NULL, 0, &n) == 1 && n == 0); free(buf);
    for (int i = 0; i < 128; ++i) assert(coglet_surface_send(link, health, health_n) == 0);
    assert(coglet_surface_send(link, health, health_n) == 1);
    for (int i = 0; i < 128; ++i) {
        assert(coglet_surface_recv(link, NULL, 0, &n) == 2);
        buf = malloc(n); assert(buf); assert(coglet_surface_recv(link, buf, n, &n) == 0); free(buf);
    }
    assert(coglet_surface_recv(link, NULL, 0, &n) == 1 && n == 0); /* blocked send had no effect */
    unsigned char bad = 0xff;
    assert(coglet_surface_send(link, &bad, 1) == -2);
    assert(coglet_surface_recv(link, NULL, 0, &n) == -2 && n == 0);
    coglet_surface_close(link); /* no calls after close */
    assert(coglet_surface_open(1, &link) == 0);
    hello = readfile(argv[2], &hello_n);
    assert(coglet_surface_send(link, hello, hello_n) == 0); free(hello);
    assert(coglet_surface_recv(link, NULL, 0, &n) == 2);
    buf = malloc(n); assert(buf); assert(coglet_surface_recv(link, buf, n, &n) == 0); free(buf);
    sleep(2);
    assert(coglet_surface_recv(link, NULL, 0, &n) == 2); /* unsolicited heartbeat while idle */
    buf = malloc(n); assert(buf); assert(coglet_surface_recv(link, buf, n, &n) == 0); free(buf);
    sleep(2);
    assert(coglet_surface_send(link, health, health_n) == -2); /* cannot revive timed-out link */
    assert(coglet_surface_recv(link, NULL, 0, &n) == -2 && n == 0);
    coglet_surface_close(link); coglet_surface_close(NULL);
    free(health);
    puts("native ABI pointer/buffer/backpressure/malformed/close probes passed");
    return 0;
}
