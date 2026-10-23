#ifndef PSPC_RNG_H
#define PSPC_RNG_H

#include <stddef.h>
#include <stdint.h>

/* Collects entropy (microphone noise when available, timer jitter, clocks).
 * Returns the number of microphone bytes mixed in (0 if the mic was unavailable). */
int rng_init(void);
/* Mixes extra unpredictable data (input timings, packet arrival times) into the pool. */
void rng_mix(const void *data, size_t n);
void rng_bytes(uint8_t *out, size_t n);

#endif
