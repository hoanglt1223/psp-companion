/* 16-pixel bitmap font (GNU Unifont subset) with Vietnamese coverage, and UTF-8 helpers.
 * Pure C99 (no PSP headers), so the host test harness can check the text layout. */
#ifndef PSPC_FONT_H
#define PSPC_FONT_H

#include <stddef.h>
#include <stdint.h>

#define FONT_HEIGHT 16

typedef struct {
    const uint8_t *bits; /* FONT_HEIGHT rows of width / 8 bytes, MSB is the leftmost pixel */
    int width;           /* 8 or 16 */
} font_glyph;

extern const unsigned font_vi_count;
extern const uint16_t font_vi_codes[];
extern const uint16_t font_vi_offsets[];
extern const uint8_t font_vi_widths[];
extern const uint8_t font_vi_bits[];

/* Decodes one code point and advances *s; invalid or truncated sequences yield U+FFFD.
 * Returns 0 at the terminating NUL (without advancing). */
uint32_t utf8_next(const char **s);

/* Missing code points get the U+FFFD glyph. */
font_glyph font_lookup(uint32_t cp);

int font_text_width(const char *s);

/* Length in bytes of the longest prefix of s that fits in max_width pixels, preferring to break after
 * a space when the whole text does not fit. Stops at '\n' (not included). Returns at least one
 * character when s is non-empty, so wrapping always makes progress. */
size_t font_fit(const char *s, int max_width);

/* Wraps s into at most max_lines lines of max_width pixels. Line i is s[starts[i]] for lens[i] bytes.
 * Returns the number of lines. */
int font_wrap(const char *s, int max_width, size_t *starts, size_t *lens, int max_lines);

#endif
