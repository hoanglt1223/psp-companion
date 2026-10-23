#include "font.h"

uint32_t utf8_next(const char **s)
{
    const unsigned char *p = (const unsigned char *)*s;
    uint32_t cp, min;
    int extra, i;
    if (p[0] == 0) return 0;
    if (p[0] < 0x80) {
        *s += 1;
        return p[0];
    }
    if ((p[0] & 0xE0) == 0xC0) {
        cp = p[0] & 0x1F;
        extra = 1;
        min = 0x80;
    } else if ((p[0] & 0xF0) == 0xE0) {
        cp = p[0] & 0x0F;
        extra = 2;
        min = 0x800;
    } else if ((p[0] & 0xF8) == 0xF0) {
        cp = p[0] & 0x07;
        extra = 3;
        min = 0x10000;
    } else {
        *s += 1;
        return 0xFFFD;
    }
    for (i = 1; i <= extra; i++) {
        if ((p[i] & 0xC0) != 0x80) {
            *s += i; /* resynchronise at the offending byte */
            return 0xFFFD;
        }
        cp = (cp << 6) | (p[i] & 0x3F);
    }
    *s += 1 + extra;
    if (cp < min || cp > 0x10FFFF || (cp >= 0xD800 && cp <= 0xDFFF)) return 0xFFFD;
    return cp;
}

static int find(uint32_t cp)
{
    int lo = 0, hi = (int)font_vi_count - 1;
    if (cp > 0xFFFF) return -1;
    while (lo <= hi) {
        int mid = (lo + hi) / 2;
        if (font_vi_codes[mid] == cp) return mid;
        if (font_vi_codes[mid] < cp)
            lo = mid + 1;
        else
            hi = mid - 1;
    }
    return -1;
}

font_glyph font_lookup(uint32_t cp)
{
    font_glyph g;
    int i = find(cp);
    if (i < 0) i = find(0xFFFD);
    if (i < 0) i = find('?');
    g.bits = font_vi_bits + font_vi_offsets[i];
    g.width = font_vi_widths[i];
    return g;
}

int font_text_width(const char *s)
{
    int w = 0;
    uint32_t cp;
    while ((cp = utf8_next(&s)) != 0) w += font_lookup(cp).width;
    return w;
}

size_t font_fit(const char *s, int max_width)
{
    const char *p = s, *last_space_end = NULL;
    int w = 0;
    for (;;) {
        const char *before = p;
        uint32_t cp = utf8_next(&p);
        if (cp == 0 || cp == '\n') return (size_t)(before - s);
        w += font_lookup(cp).width;
        if (w > max_width) {
            if (last_space_end) return (size_t)(last_space_end - s);
            if (before == s) return (size_t)(p - s);
            return (size_t)(before - s);
        }
        if (cp == ' ') last_space_end = p;
    }
}

int font_wrap(const char *s, int max_width, size_t *starts, size_t *lens, int max_lines)
{
    size_t pos = 0;
    int n = 0;
    while (n < max_lines && s[pos]) {
        size_t len = font_fit(s + pos, max_width);
        starts[n] = pos;
        lens[n] = len;
        n++;
        pos += len;
        if (s[pos] == '\n')
            pos++;
        else
            while (s[pos] == ' ') pos++;
    }
    return n;
}
