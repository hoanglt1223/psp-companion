#include "macros.h"

#include <pspctrl.h>
#include <stdio.h>
#include <string.h>

#include "font.h"
#include "gfx.h"
#include "net.h"

#define COLS 4
#define ROWS 3
#define CELL_W 112
#define CELL_H 70
#define GRID_X 16
#define GRID_Y 36
#define FLASH_US (200 * 1000ULL)

static char g_labels[PSPC_MAX_MACROS][25];
static int g_open, g_sel;
static int g_flash_slot = -1;
static uint64_t g_flash_until;

void macropad_set_labels(const pspc_layout *layout)
{
    int i;
    memset(g_labels, 0, sizeof g_labels);
    for (i = 0; i < layout->macro_count; i++)
        snprintf(g_labels[layout->macros[i].slot], sizeof g_labels[0], "%s", layout->macros[i].label);
}

void macropad_open(void)
{
    g_open = 1;
}

void macropad_close(void)
{
    g_open = 0;
}

int macropad_is_open(void)
{
    return g_open;
}

static int cell_at(int x, int y)
{
    int c = (x - GRID_X) / CELL_W, r = (y - GRID_Y) / CELL_H;
    if (x < GRID_X || y < GRID_Y || c >= COLS || r >= ROWS) return -1;
    return r * COLS + c;
}

static void trigger(int slot, uint64_t now_us)
{
    uint8_t m[4];
    if (!g_labels[slot][0]) return;
    if (net_send(m, pspc_build_macro_trigger(m, (uint8_t)slot)) == 0) {
        g_flash_slot = slot;
        g_flash_until = now_us + FLASH_US;
    }
}

void macropad_input(uint32_t pressed, int cursor_visible, int cx, int cy, uint64_t now_us)
{
    int r = g_sel / COLS, c = g_sel % COLS;
    if (pressed & (PSP_CTRL_CIRCLE | PSP_CTRL_TRIANGLE)) {
        g_open = 0;
        return;
    }
    if (pressed & PSP_CTRL_LEFT) c = (c + COLS - 1) % COLS;
    if (pressed & PSP_CTRL_RIGHT) c = (c + 1) % COLS;
    if (pressed & PSP_CTRL_UP) r = (r + ROWS - 1) % ROWS;
    if (pressed & PSP_CTRL_DOWN) r = (r + 1) % ROWS;
    g_sel = r * COLS + c;
    if (cursor_visible) {
        int hit = cell_at(cx, cy);
        if (hit >= 0) g_sel = hit;
    }
    if (pressed & PSP_CTRL_CROSS) trigger(g_sel, now_us);
}

void macropad_draw(uint64_t now_us)
{
    int i;
    gfx_fill(0, 20, 480, 252, GFX_BG);
    gfx_text(16, 18, GFX_ACCENT, "Macros", 0);
    gfx_text(200, 18, GFX_DIM, "X run   O close   D-pad/nub select", 0);
    for (i = 0; i < PSPC_MAX_MACROS; i++) {
        int x = GRID_X + (i % COLS) * CELL_W, y = GRID_Y + (i / COLS) * CELL_H;
        int flash = i == g_flash_slot && now_us < g_flash_until;
        uint32_t bg = flash ? GFX_ACCENT : GFX_PANEL;
        gfx_fill(x + 2, y + 2, CELL_W - 4, CELL_H - 4, bg);
        gfx_frame(x + 2, y + 2, CELL_W - 4, CELL_H - 4, i == g_sel ? GFX_TEXT : GFX_FRAME);
        if (i == g_sel) gfx_frame(x + 3, y + 3, CELL_W - 6, CELL_H - 6, GFX_TEXT);
        if (g_labels[i][0])
            gfx_wrap(x + 8, y + 12, CELL_W - 16, 3, flash ? 0xFF000000 : GFX_TEXT, g_labels[i]);
        else
            gfx_text(x + 8, y + 12, GFX_DIM, "(empty)", CELL_W - 16);
    }
}
