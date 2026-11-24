#include <stddef.h>
#include <stdint.h>
extern int32_t coglet_runtime_configure(const uint8_t *, size_t);
extern int32_t coglet_runtime_start(const uint8_t *, size_t);
extern int32_t coglet_runtime_stop(void);
/* Force real archive symbol resolution, including the WAMR/clock dependencies.
 * Not run: configuration/start arguments are intentionally null in this link probe. */
int main(void) {
    int32_t result = coglet_runtime_configure(NULL, 0);
    result += coglet_runtime_start(NULL, 0);
    return result + coglet_runtime_stop();
}
