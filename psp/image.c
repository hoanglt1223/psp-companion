#include "image.h"

#include "protocol.h"

int lz4_block_decode(const uint8_t *in, size_t in_len, uint8_t *out, size_t out_len)
{
    size_t ip = 0, op = 0;
    while (ip < in_len) {
        unsigned token = in[ip++];
        size_t lit = token >> 4, match, offset, i;
        if (lit == 15) {
            unsigned b;
            do {
                if (ip >= in_len) return -1;
                b = in[ip++];
                lit += b;
            } while (b == 255);
        }
        if (lit > in_len - ip || lit > out_len - op) return -1;
        for (i = 0; i < lit; i++) out[op++] = in[ip++];
        if (ip == in_len) break; /* the last sequence has literals only */
        if (in_len - ip < 2) return -1;
        offset = (size_t)in[ip] | ((size_t)in[ip + 1] << 8);
        ip += 2;
        if (offset == 0 || offset > op) return -1;
        match = token & 15;
        if (match == 15) {
            unsigned b;
            do {
                if (ip >= in_len) return -1;
                b = in[ip++];
                match += b;
            } while (b == 255);
        }
        match += 4;
        if (match > out_len - op) return -1;
        for (i = 0; i < match; i++, op++) out[op] = out[op - offset]; /* overlapping copies are intended */
    }
    return op == out_len ? 0 : -1;
}

int image_decode(const uint8_t *payload, size_t len, uint8_t *scratch, uint32_t *out, size_t max_pixels, int *width,
                 int *height)
{
    size_t raw_len, colors, pixels, i;
    const uint8_t *pal, *idx;
    int w, h;
    if (len < 4) return -1;
    raw_len = pspc_get32(payload);
    if (raw_len < 6 || raw_len > IMAGE_MAX_RAW) return -1;
    if (lz4_block_decode(payload + 4, len - 4, scratch, raw_len) != 0) return -1;
    w = pspc_get16(scratch);
    h = pspc_get16(scratch + 2);
    colors = pspc_get16(scratch + 4);
    if (w < 1 || h < 1 || w > IMAGE_MAX_DIM || h > IMAGE_MAX_DIM || colors < 1 || colors > 256) return -1;
    pixels = (size_t)w * (size_t)h;
    if (raw_len != 6 + colors * 3 + pixels || pixels > max_pixels) return -1;
    pal = scratch + 6;
    idx = pal + colors * 3;
    for (i = 0; i < pixels; i++) {
        const uint8_t *c;
        if (idx[i] >= colors) return -1;
        c = pal + idx[i] * 3;
        out[i] = 0xFF000000u | ((uint32_t)c[2] << 16) | ((uint32_t)c[1] << 8) | c[0];
    }
    *width = w;
    *height = h;
    return 0;
}
