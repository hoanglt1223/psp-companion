/* Image format 1 (IDX8_LZ4, see protocol/PROTOCOL.md): an LZ4 block of a palettised bitmap.
 * Pure C99 (no PSP headers), so the host test harness can check it against the Rust encoder. */
#ifndef PSPC_IMAGE_H
#define PSPC_IMAGE_H

#include <stddef.h>
#include <stdint.h>

#define IMAGE_FORMAT_IDX8_LZ4 1
#define IMAGE_MAX_DIM 128
#define IMAGE_MAX_RAW (6 + 256 * 3 + IMAGE_MAX_DIM * IMAGE_MAX_DIM)

/* Decompresses an LZ4 block into exactly out_len bytes. Returns 0 on success, -1 if malformed. */
int lz4_block_decode(const uint8_t *in, size_t in_len, uint8_t *out, size_t out_len);

/* Decodes a format-1 payload into 32-bit pixels 0xAABBGGRR (the PSP framebuffer order), row by row.
 * scratch must hold IMAGE_MAX_RAW bytes; out must hold max_pixels pixels.
 * Returns 0 and the size on success, -1 if malformed or larger than max_pixels. */
int image_decode(const uint8_t *payload, size_t len, uint8_t *scratch, uint32_t *out, size_t max_pixels, int *width,
                 int *height);

#endif
