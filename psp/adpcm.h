/* IMA ADPCM encoder for VoiceChunk (protocol/PROTOCOL.md "Voice assistant"). Each chunk starts with the
 * encoder state, so the host can decode chunks independently. Pure C99 (no PSP headers). */
#ifndef PSPC_ADPCM_H
#define PSPC_ADPCM_H

#include <stddef.h>
#include <stdint.h>

#define ADPCM_HEADER 4

typedef struct {
    int32_t predictor;
    int32_t index;
} adpcm_state;

void adpcm_init(adpcm_state *st);
/* Encodes n samples (an odd count repeats the last sample) into out, which must hold
 * ADPCM_HEADER + (n + 1) / 2 bytes. The state carries over to the next chunk. Returns the bytes written. */
size_t adpcm_encode_chunk(adpcm_state *st, const int16_t *pcm, size_t n, uint8_t *out);

#endif
