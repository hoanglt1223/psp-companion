/* CPU drawing into the current draw buffer (between ui_begin and ui_end): rectangles, the
 * Vietnamese-capable 16 px font, and decoded images. Everything clips to the screen. */
#ifndef PSPC_GFX_H
#define PSPC_GFX_H

#include <stdint.h>

/* Colours are 0xAABBGGRR, like ui.h. */
#define GFX_BG 0xFF302418
#define GFX_PANEL 0xFF4A3828
#define GFX_FRAME 0xFF7A6450
#define GFX_ACCENT 0xFFF0C040
#define GFX_TEXT 0xFFFFFFFF
#define GFX_DIM 0xFFA0A0A0
#define GFX_GOOD 0xFF60E060
#define GFX_WARN 0xFF40E0F0
#define GFX_BAD 0xFF6060FF

void gfx_fill(int x, int y, int w, int h, uint32_t color);
void gfx_frame(int x, int y, int w, int h, uint32_t color);
/* Draws one line of UTF-8 text, cut at max_w pixels (0: no limit) and at '\n'. Returns the width drawn. */
int gfx_text(int x, int y, uint32_t color, const char *s, int max_w);
/* Same, with each font pixel drawn as scale x scale. */
int gfx_text_scaled(int x, int y, int scale, uint32_t color, const char *s);
/* Word-wraps into at most max_lines lines of FONT_HEIGHT; returns the lines drawn. */
int gfx_wrap(int x, int y, int w, int max_lines, uint32_t color, const char *s);
void gfx_blit(int x, int y, int w, int h, const uint32_t *pixels);
/* A horizontal bar filled to pct (0-100). */
void gfx_bar(int x, int y, int w, int h, int pct, uint32_t color);

#endif
