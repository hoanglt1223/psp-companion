#include "streamdeck.h"

#include <pspctrl.h>
#include <string.h>

#include "cursor.h"
#include "gfx.h"
#include "image.h"
#include "macros.h"
#include "net.h"
#include "protocol.h"

#define MAX_KEYS 32
#define MAX_DIM 96
#define CACHE 40 /* more than two pages of 18 keys, so flipping pages back costs no image data */
#define TOP 20

static struct {
    uint64_t id;
    int w, h;
    uint32_t stamp;
} g_cache[CACHE];
static uint32_t g_cache_px[CACHE][MAX_DIM * MAX_DIM];
static uint32_t g_clock;
static uint8_t g_scratch[IMAGE_MAX_RAW];

static int g_cols, g_rows, g_have_layout;
static uint64_t g_key_image[MAX_KEYS];
static int g_sel;
static int g_held = -1; /* key whose press was sent and whose release is pending */

void deck_reset(void)
{
    deck_release();
    g_have_layout = 0;
    g_cols = g_rows = 0;
    memset(g_key_image, 0, sizeof g_key_image);
    macropad_close();
}

static int find(uint64_t id)
{
    int i;
    for (i = 0; i < CACHE; i++)
        if (id != 0 && g_cache[i].id == id) return i;
    return -1;
}

static int in_use(uint64_t id)
{
    int k;
    for (k = 0; k < g_cols * g_rows; k++)
        if (g_key_image[k] == id) return 1;
    return 0;
}

/* Evicts the least recently stored image that no key shows right now. */
static int victim(void)
{
    int i, best = -1;
    for (i = 0; i < CACHE; i++) {
        if (g_cache[i].id == 0) return i;
        if (in_use(g_cache[i].id)) continue;
        if (best < 0 || g_cache[i].stamp < g_cache[best].stamp) best = i;
    }
    return best < 0 ? 0 : best;
}

int deck_on_message(const uint8_t *msg, size_t len)
{
    uint8_t cols, rows, kw, kh, key, format, w, h;
    uint64_t id;
    const uint8_t *data;
    size_t n;
    int slot, iw, ih;
    switch (msg[0]) {
    case PSPC_MSG_DECK_LAYOUT:
        if (pspc_parse_deck_layout(msg, len, &cols, &rows, &kw, &kh) != PSPC_OK) return 1;
        deck_release();
        g_have_layout = 1;
        /* 0x0 means Bitfocus Companion is not reachable; keep the grid within what this screen can show. */
        if (cols * rows > MAX_KEYS || kw > MAX_DIM || kh > MAX_DIM) cols = rows = 0;
        g_cols = cols;
        g_rows = rows;
        memset(g_key_image, 0, sizeof g_key_image);
        if (g_sel >= g_cols * g_rows) g_sel = 0;
        return 1;
    case PSPC_MSG_DECK_KEY_IMAGE:
        if (pspc_parse_deck_key_image(msg, len, &key, &id, &format, &w, &h, &data, &n) != PSPC_OK) return 1;
        if (key >= g_cols * g_rows || format != IMAGE_FORMAT_IDX8_LZ4) return 1;
        if (n > 0 && find(id) < 0) {
            slot = victim();
            if (image_decode(data, n, g_scratch, g_cache_px[slot], MAX_DIM * MAX_DIM, &iw, &ih) != 0) return 1;
            g_cache[slot].id = id;
            g_cache[slot].w = iw;
            g_cache[slot].h = ih;
            g_cache[slot].stamp = ++g_clock;
        }
        g_key_image[key] = id;
        return 1;
    default:
        return 0;
    }
}

static int cell_w(void)
{
    return g_cols ? UI_WIDTH / g_cols : UI_WIDTH;
}

static int cell_h(void)
{
    return g_rows ? (UI_HEIGHT - TOP) / g_rows : UI_HEIGHT - TOP;
}

static void send_key(int key, int pressed)
{
    uint8_t m[4];
    net_send(m, pspc_build_deck_key_press(m, (uint8_t)key, (uint8_t)pressed));
}

void deck_release(void)
{
    if (g_held >= 0) send_key(g_held, 0);
    g_held = -1;
}

void deck_input(uint32_t pressed, const ui_pad *pad, uint64_t now_us)
{
    int keys = g_cols * g_rows, r, c;
    cursor_update(pad, now_us);
    if (macropad_is_open()) {
        macropad_input(pressed, cursor_visible(), cursor_x(), cursor_y(), now_us);
        return;
    }
    if (pressed & PSP_CTRL_TRIANGLE) {
        deck_release();
        macropad_open();
        return;
    }
    if (keys == 0) return;
    if (pressed & PSP_CTRL_LTRIGGER) send_key(PSPC_DECK_KEY_PAGE_PREV, 1);
    if (pressed & PSP_CTRL_RTRIGGER) send_key(PSPC_DECK_KEY_PAGE_NEXT, 1);
    r = g_sel / g_cols;
    c = g_sel % g_cols;
    if (g_held < 0) {
        if (pressed & PSP_CTRL_LEFT) c = (c + g_cols - 1) % g_cols;
        if (pressed & PSP_CTRL_RIGHT) c = (c + 1) % g_cols;
        if (pressed & PSP_CTRL_UP) r = (r + g_rows - 1) % g_rows;
        if (pressed & PSP_CTRL_DOWN) r = (r + 1) % g_rows;
        g_sel = r * g_cols + c;
        if (cursor_visible() && cursor_y() >= TOP) {
            int hc = cursor_x() / cell_w(), hr = (cursor_y() - TOP) / cell_h();
            if (hc < g_cols && hr < g_rows) g_sel = hr * g_cols + hc;
        }
    }
    if ((pressed & PSP_CTRL_CROSS) && g_held < 0) {
        g_held = g_sel;
        send_key(g_held, 1);
    } else if (g_held >= 0 && !(pad->buttons & PSP_CTRL_CROSS)) {
        deck_release();
    }
}

void deck_draw(uint64_t now_us)
{
    int k, keys = g_cols * g_rows, cw = cell_w(), ch = cell_h();
    gfx_fill(0, 0, UI_WIDTH, UI_HEIGHT, 0xFF000000);
    if (macropad_is_open()) {
        macropad_draw(now_us);
        cursor_draw();
        return;
    }
    gfx_text(8, 2, GFX_ACCENT, "Stream Deck", 0);
    if (!g_have_layout || keys == 0) {
        gfx_wrap(16, 60, UI_WIDTH - 32, 4, GFX_TEXT,
                 g_have_layout ? "Bitfocus Companion is not connected on the PC. Start it, or press Triangle for "
                                 "the built-in macro pad."
                               : "Waiting for the PC...");
        return;
    }
    gfx_text(150, 2, GFX_DIM, "X press  L/R page  \xE2\x96\xB3 macros  START menu", 0);
    for (k = 0; k < keys; k++) {
        int x = (k % g_cols) * cw, y = TOP + (k / g_cols) * ch, slot = find(g_key_image[k]);
        uint32_t frame = k == g_held ? GFX_TEXT : k == g_sel ? GFX_ACCENT : 0xFF202020;
        if (slot >= 0) {
            int iw = g_cache[slot].w, ih = g_cache[slot].h;
            gfx_blit(x + (cw - iw) / 2, y + (ch - ih) / 2, iw, ih, g_cache_px[slot]);
        }
        gfx_frame(x + 1, y + 1, cw - 2, ch - 2, frame);
        if (k == g_sel || k == g_held) gfx_frame(x + 2, y + 2, cw - 4, ch - 4, frame);
    }
    cursor_draw();
}
