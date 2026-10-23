#include "adpcm.h"

static const int16_t STEPS[89] = {
    7,     8,     9,     10,    11,    12,    13,    14,    16,    17,    19,    21,    23,    25,    28,
    31,    34,    37,    41,    45,    50,    55,    60,    66,    73,    80,    88,    97,    107,   118,
    130,   143,   157,   173,   190,   209,   230,   253,   279,   307,   337,   371,   408,   449,   494,
    544,   598,   658,   724,   796,   876,   963,   1060,  1166,  1282,  1411,  1552,  1707,  1878,  2066,
    2272,  2499,  2749,  3024,  3327,  3660,  4026,  4428,  4871,  5358,  5894,  6484,  7132,  7845,  8630,
    9493,  10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767};
static const int8_t INDEX_ADJUST[8] = {-1, -1, -1, -1, 2, 4, 6, 8};

void adpcm_init(adpcm_state *st)
{
    st->predictor = 0;
    st->index = 0;
}

/* Mirrors the decoder so the encoder tracks exactly what the host reconstructs. */
static void step(adpcm_state *st, uint8_t code)
{
    int32_t s = STEPS[st->index], diff = s >> 3;
    if (code & 4) diff += s;
    if (code & 2) diff += s >> 1;
    if (code & 1) diff += s >> 2;
    st->predictor += (code & 8) ? -diff : diff;
    if (st->predictor > 32767) st->predictor = 32767;
    if (st->predictor < -32768) st->predictor = -32768;
    st->index += INDEX_ADJUST[code & 7];
    if (st->index < 0) st->index = 0;
    if (st->index > 88) st->index = 88;
}

static uint8_t encode(adpcm_state *st, int16_t sample)
{
    int32_t s = STEPS[st->index], diff = (int32_t)sample - st->predictor;
    uint8_t code = 0;
    if (diff < 0) {
        code = 8;
        diff = -diff;
    }
    if (diff >= s) {
        code |= 4;
        diff -= s;
    }
    s >>= 1;
    if (diff >= s) {
        code |= 2;
        diff -= s;
    }
    s >>= 1;
    if (diff >= s) code |= 1;
    step(st, code);
    return code;
}

size_t adpcm_encode_chunk(adpcm_state *st, const int16_t *pcm, size_t n, uint8_t *out)
{
    size_t i, o = ADPCM_HEADER;
    out[0] = (uint8_t)(st->predictor & 0xFF);
    out[1] = (uint8_t)((st->predictor >> 8) & 0xFF);
    out[2] = (uint8_t)st->index;
    out[3] = 0;
    for (i = 0; i < n; i += 2) {
        uint8_t lo = encode(st, pcm[i]);
        uint8_t hi = encode(st, i + 1 < n ? pcm[i + 1] : pcm[i]);
        out[o++] = (uint8_t)(lo | (hi << 4));
    }
    return o;
}
