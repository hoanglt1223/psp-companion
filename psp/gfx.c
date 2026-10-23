#include "gfx.h"

#include <string.h>

#include "font.h"
#include "ui.h"

static void clip(int *x, int *y, int *w, int *h)
{
    if (*x < 0) {
        *w += *x;
        *x = 0;
    }
    if (*y < 0) {
        *h += *y;
        *y = 0;
    }
    if (*x + *w > UI_WIDTH) *w = UI_WIDTH - *x;
    if (*y + *h > UI_HEIGHT) *h = UI_HEIGHT - *y;
}

void gfx_fill(int x, int y, int w, int h, uint32_t color)
{
    uint32_t *fb = ui_framebuffer();
    int i, j;
    clip(&x, &y, &w, &h);
    for (j = 0; j < h; j++) {
        uint32_t *row = fb + (y + j) * UI_STRIDE + x;
        for (i = 0; i < w; i++) row[i] = color;
    }
}

void gfx_frame(int x, int y, int w, int h, uint32_t color)
{
    gfx_fill(x, y, w, 1, color);
    gfx_fill(x, y + h - 1, w, 1, color);
    gfx_fill(x, y, 1, h, color);
    gfx_fill(x + w - 1, y, 1, h, color);
}

static void glyph(uint32_t *fb, int x, int y, int scale, uint32_t color, font_glyph g)
{
    int bpr = g.width / 8, row, col, sy, sx;
    for (row = 0; row < FONT_HEIGHT; row++) {
        for (col = 0; col < g.width; col++) {
            if (!(g.bits[row * bpr + col / 8] & (0x80 >> (col % 8)))) continue;
            for (sy = 0; sy < scale; sy++) {
                int py = y + row * scale + sy;
                if (py < 0 || py >= UI_HEIGHT) continue;
                for (sx = 0; sx < scale; sx++) {
                    int px = x + col * scale + sx;
                    if (px >= 0 && px < UI_WIDTH) fb[py * UI_STRIDE + px] = color;
                }
            }
        }
    }
}

static int text(int x, int y, int scale, uint32_t color, const char *s, int max_w)
{
    uint32_t *fb = ui_framebuffer();
    int w = 0;
    uint32_t cp;
    while ((cp = utf8_next(&s)) != 0 && cp != '\n') {
        font_glyph g = font_lookup(cp);
        if (max_w > 0 && w + g.width * scale > max_w) break;
        glyph(fb, x + w, y, scale, color, g);
        w += g.width * scale;
    }
    return w;
}

int gfx_text(int x, int y, uint32_t color, const char *s, int max_w)
{
    return text(x, y, 1, color, s, max_w);
}

int gfx_text_scaled(int x, int y, int scale, uint32_t color, const char *s)
{
    return text(x, y, scale, color, s, 0);
}

int gfx_wrap(int x, int y, int w, int max_lines, uint32_t color, const char *s)
{
    size_t starts[16], lens[16];
    char line[256];
    int n, i;
    if (max_lines > 16) max_lines = 16;
    n = font_wrap(s, w, starts, lens, max_lines);
    for (i = 0; i < n; i++) {
        size_t len = lens[i] < sizeof line - 1 ? lens[i] : sizeof line - 1;
        memcpy(line, s + starts[i], len);
        line[len] = '\0';
        gfx_text(x, y + i * FONT_HEIGHT, color, line, w);
    }
    return n;
}

void gfx_blit(int x, int y, int w, int h, const uint32_t *pixels)
{
    uint32_t *fb = ui_framebuffer();
    int i, j;
    for (j = 0; j < h; j++) {
        int py = y + j;
        if (py < 0 || py >= UI_HEIGHT) continue;
        for (i = 0; i < w; i++) {
            int px = x + i;
            if (px >= 0 && px < UI_WIDTH) fb[py * UI_STRIDE + px] = pixels[j * w + i];
        }
    }
}

void gfx_bar(int x, int y, int w, int h, int pct, uint32_t color)
{
    if (pct < 0) pct = 0;
    if (pct > 100) pct = 100;
    gfx_fill(x, y, w, h, GFX_PANEL);
    gfx_fill(x, y, w * pct / 100, h, color);
    gfx_frame(x, y, w, h, GFX_FRAME);
}
