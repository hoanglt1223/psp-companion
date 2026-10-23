#include "widgets.h"

#include <pspctrl.h>
#include <psprtc.h>
#include <stdio.h>
#include <string.h>

#include "cursor.h"
#include "font.h"
#include "gfx.h"
#include "image.h"
#include "macros.h"
#include "net.h"
#include "protocol.h"

#define ASSET_SLOTS 4
#define ASSET_PIXELS (IMAGE_MAX_DIM * IMAGE_MAX_DIM)
#define TAP_FLASH_US (150 * 1000ULL)

static pspc_layout g_layout;
static int g_have_layout;
static pspc_widget_value g_values[PSPC_MAX_WIDGETS];
static uint8_t g_has_value[PSPC_MAX_WIDGETS];
static int g_page;
static int g_flash_widget = -1;
static uint64_t g_flash_until;

static struct {
    uint64_t id;
    int w, h;
    uint32_t stamp;
} g_assets[ASSET_SLOTS];
static uint32_t g_asset_px[ASSET_SLOTS][ASSET_PIXELS];
static uint32_t g_asset_clock;
static uint8_t g_scratch[IMAGE_MAX_RAW];

void widgets_reset(void)
{
    g_have_layout = 0;
    memset(g_has_value, 0, sizeof g_has_value);
    g_page = 0;
    macropad_close();
    cursor_reset();
}

static int widget_index(uint16_t id)
{
    int i;
    for (i = 0; i < g_layout.widget_count; i++)
        if (g_layout.widgets[i].id == id) return i;
    return -1;
}

/* Least recently stored slot is replaced; a cover changes at most once per track. */
static void store_asset(uint64_t id, const uint8_t *data, size_t len)
{
    int i, victim = 0, w, h;
    for (i = 0; i < ASSET_SLOTS; i++) {
        if (g_assets[i].id == id) return;
        if (g_assets[i].stamp < g_assets[victim].stamp) victim = i;
    }
    if (image_decode(data, len, g_scratch, g_asset_px[victim], ASSET_PIXELS, &w, &h) != 0) return;
    g_assets[victim].id = id;
    g_assets[victim].w = w;
    g_assets[victim].h = h;
    g_assets[victim].stamp = ++g_asset_clock;
}

static const uint32_t *find_asset(uint64_t id, int *w, int *h)
{
    int i;
    if (id == 0) return NULL;
    for (i = 0; i < ASSET_SLOTS; i++) {
        if (g_assets[i].id == id) {
            *w = g_assets[i].w;
            *h = g_assets[i].h;
            return g_asset_px[i];
        }
    }
    return NULL;
}

int widgets_on_message(const uint8_t *msg, size_t len)
{
    const uint8_t *data;
    size_t data_len;
    uint8_t format;
    uint16_t id;
    uint64_t asset_id;
    int i;
    switch (msg[0]) {
    case PSPC_MSG_WIDGET_LAYOUT:
        if (pspc_parse_widget_layout(msg, len, &format, &data, &data_len) == PSPC_OK && format == PSPC_FORMAT_BINARY &&
            pspc_parse_layout(data, data_len, &g_layout) == PSPC_OK) {
            g_have_layout = 1;
            memset(g_has_value, 0, sizeof g_has_value);
            if (g_page >= g_layout.page_count) g_page = 0;
            macropad_set_labels(&g_layout);
        }
        return 1;
    case PSPC_MSG_WIDGET_DATA:
        if (pspc_parse_widget_data(msg, len, &id, &format, &data, &data_len) != PSPC_OK ||
            format != PSPC_FORMAT_BINARY || (i = widget_index(id)) < 0)
            return 1;
        if (pspc_parse_widget_value(g_layout.widgets[i].kind, data, data_len, &g_values[i]) == PSPC_OK)
            g_has_value[i] = 1;
        return 1;
    case PSPC_MSG_WIDGET_ASSET:
        if (pspc_parse_widget_asset(msg, len, &asset_id, &format, &data, &data_len) == PSPC_OK &&
            format == IMAGE_FORMAT_IDX8_LZ4)
            store_asset(asset_id, data, data_len);
        return 1;
    default:
        return 0;
    }
}

static void tap(int cx, int cy, uint64_t now_us)
{
    int i;
    for (i = 0; i < g_layout.widget_count; i++) {
        const pspc_widget_def *d = &g_layout.widgets[i];
        uint8_t m[16];
        if (d->page != g_page || cx < d->x || cy < d->y || cx >= d->x + d->w || cy >= d->y + d->h) continue;
        if (!(d->flags & PSPC_WIDGET_TAPPABLE)) return;
        if (net_send(m, pspc_build_input_event(m, PSPC_INPUT_TAP, (uint8_t)g_page, d->id, (uint16_t)cx, (uint16_t)cy,
                                               0)) == 0) {
            g_flash_widget = i;
            g_flash_until = now_us + TAP_FLASH_US;
        }
        return;
    }
}

void widgets_input(uint32_t pressed, const ui_pad *pad, uint64_t now_us)
{
    cursor_update(pad, now_us);
    if (macropad_is_open()) {
        macropad_input(pressed, cursor_visible(), cursor_x(), cursor_y(), now_us);
        return;
    }
    if (pressed & PSP_CTRL_TRIANGLE) {
        macropad_open();
        return;
    }
    if (g_have_layout && g_layout.page_count > 0) {
        if (pressed & PSP_CTRL_LTRIGGER) g_page = (g_page + g_layout.page_count - 1) % g_layout.page_count;
        if (pressed & PSP_CTRL_RTRIGGER) g_page = (g_page + 1) % g_layout.page_count;
    }
    if ((pressed & PSP_CTRL_CROSS) && cursor_visible()) tap(cursor_x(), cursor_y(), now_us);
}

/* ---------- drawing ---------- */

static void draw_clock(const pspc_widget_def *d)
{
    static const char *days[] = {"Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"};
    ScePspDateTime t;
    char line[32];
    int dow;
    if (sceRtcGetCurrentClockLocalTime(&t) < 0) return;
    snprintf(line, sizeof line, "%02u:%02u", (unsigned)t.hour, (unsigned)t.minute);
    gfx_text_scaled(d->x + 12, d->y + 24, 3, GFX_TEXT, line);
    dow = sceRtcGetDayOfWeek(t.year, t.month, t.day);
    snprintf(line, sizeof line, "%s %02u/%02u/%04u", days[dow >= 0 && dow < 7 ? dow : 0], (unsigned)t.day,
             (unsigned)t.month, (unsigned)t.year);
    gfx_text(d->x + 12, d->y + 24 + 3 * FONT_HEIGHT + 4, GFX_DIM, line, d->w - 16);
}

static void temp_text(char *out, size_t cap, const char *label, int16_t t)
{
    if (t == PSPC_TEMP_UNKNOWN)
        out[0] = '\0';
    else
        snprintf(out, cap, "%s %d.%d\xC2\xB0" "C", label, t / 10, (t < 0 ? -t : t) % 10);
}

static void draw_system(const pspc_widget_def *d, const pspc_w_system *s)
{
    char line[64], temps[2][24];
    int x = d->x + 8, y = d->y + 20, bw = d->w - 16 - 72;
    snprintf(line, sizeof line, "CPU %3u%%", (unsigned)s->cpu_pct);
    gfx_text(x, y, GFX_TEXT, line, 0);
    gfx_bar(x + 72, y + 3, bw, 10, s->cpu_pct, s->cpu_pct > 85 ? GFX_BAD : GFX_GOOD);
    y += 20;
    if (s->gpu_pct != PSPC_PCT_UNKNOWN) {
        snprintf(line, sizeof line, "GPU %3u%%", (unsigned)s->gpu_pct);
        gfx_text(x, y, GFX_TEXT, line, 0);
        gfx_bar(x + 72, y + 3, bw, 10, s->gpu_pct, s->gpu_pct > 85 ? GFX_BAD : GFX_GOOD);
        y += 20;
    }
    if (s->ram_total_mb > 0) {
        int pct = (int)((uint64_t)s->ram_used_mb * 100 / s->ram_total_mb);
        snprintf(line, sizeof line, "RAM %3d%%", pct);
        gfx_text(x, y, GFX_TEXT, line, 0);
        gfx_bar(x + 72, y + 3, bw, 10, pct, pct > 90 ? GFX_BAD : GFX_ACCENT);
        y += 20;
        snprintf(line, sizeof line, "%u.%u / %u.%u GB", (unsigned)(s->ram_used_mb / 1024),
                 (unsigned)(s->ram_used_mb % 1024 * 10 / 1024), (unsigned)(s->ram_total_mb / 1024),
                 (unsigned)(s->ram_total_mb % 1024 * 10 / 1024));
        gfx_text(x, y, GFX_DIM, line, d->w - 16);
        y += 18;
    }
    temp_text(temps[0], sizeof temps[0], "CPU", s->cpu_temp);
    temp_text(temps[1], sizeof temps[1], "GPU", s->gpu_temp);
    snprintf(line, sizeof line, "%s%s%s", temps[0], temps[0][0] && temps[1][0] ? "  " : "", temps[1]);
    if (line[0] && y + FONT_HEIGHT <= d->y + d->h) gfx_text(x, y, GFX_DIM, line, d->w - 16);
}

static void draw_now_playing(const pspc_widget_def *d, const pspc_w_now_playing *n)
{
    static const char *states[] = {"", "\xE2\x96\xB6 Playing", "\xE2\x96\xA0 Paused", "\xE2\x96\xA0 Stopped"};
    int w, h, tx = d->x + 8;
    const uint32_t *px = find_asset(n->cover, &w, &h);
    if (n->state == 0) {
        gfx_text(d->x + 8, d->y + 24, GFX_DIM, "Nothing playing", d->w - 16);
        return;
    }
    if (px && d->w > w + 24 && d->h > h + 24) {
        gfx_blit(d->x + 8, d->y + 24, w, h, px);
        tx += w + 8;
    }
    gfx_wrap(tx, d->y + 22, d->x + d->w - 8 - tx, 2, GFX_TEXT, n->title);
    gfx_text(tx, d->y + 22 + 2 * FONT_HEIGHT + 2, GFX_DIM, n->artist, d->x + d->w - 8 - tx);
    gfx_text(tx, d->y + d->h - FONT_HEIGHT - 6, n->state == 1 ? GFX_GOOD : GFX_WARN, states[n->state > 3 ? 0 : n->state],
             0);
}

static void draw_notifications(const pspc_widget_def *d, const pspc_w_notifications *n)
{
    int i, y = d->y + 22;
    if (n->count == 0) {
        gfx_text(d->x + 8, y, GFX_DIM, "No notifications", d->w - 16);
        return;
    }
    for (i = 0; i < n->count && y + 2 * FONT_HEIGHT <= d->y + d->h; i++) {
        char head[128];
        int tw = gfx_text(d->x + 8, y, GFX_DIM, n->items[i].time, 0);
        snprintf(head, sizeof head, "%s%s%s", n->items[i].app, n->items[i].app[0] ? ": " : "", n->items[i].title);
        gfx_text(d->x + 16 + tw, y, GFX_ACCENT, head, d->w - 24 - tw);
        y += FONT_HEIGHT;
        if (n->items[i].body[0]) {
            int lines = (d->y + d->h - y) / FONT_HEIGHT;
            y += FONT_HEIGHT * gfx_wrap(d->x + 16, y, d->w - 24, lines < 2 ? lines : 2, GFX_TEXT, n->items[i].body);
        }
        y += 4;
    }
}

static void draw_calendar(const pspc_widget_def *d, const pspc_w_calendar *c)
{
    if (!c->has_event) {
        gfx_text(d->x + 8, d->y + 24, GFX_DIM, "No upcoming events", d->w - 16);
        return;
    }
    gfx_text(d->x + 8, d->y + 24, GFX_ACCENT, c->when, d->w - 16);
    gfx_wrap(d->x + 8, d->y + 24 + FONT_HEIGHT + 4, d->w - 16, (d->h - 52) / FONT_HEIGHT, GFX_TEXT, c->title);
}

static void draw_claude(const pspc_widget_def *d, const pspc_w_claude *c)
{
    static const char *names[] = {"idle", "working", "needs you", "done"};
    static const uint32_t colors[] = {GFX_DIM, GFX_ACCENT, GFX_BAD, GFX_GOOD};
    int i, y = d->y + 22;
    if (c->count == 0) {
        gfx_text(d->x + 8, y, GFX_DIM, "No Claude Code sessions", d->w - 16);
        return;
    }
    for (i = 0; i < c->count && y + 2 * FONT_HEIGHT <= d->y + d->h; i++) {
        int s = c->items[i].state > 3 ? 0 : c->items[i].state;
        int pw = gfx_text(d->x + 8, y, GFX_TEXT, c->items[i].project, d->w / 2);
        gfx_text(d->x + 16 + pw, y, colors[s], names[s], 0);
        gfx_text(d->x + 16, y + FONT_HEIGHT, GFX_DIM, c->items[i].detail, d->w - 24);
        y += 2 * FONT_HEIGHT + 6;
    }
}

static void draw_widget(int i, uint64_t now_us)
{
    const pspc_widget_def *d = &g_layout.widgets[i];
    const pspc_widget_value *v = &g_values[i];
    int flash = i == g_flash_widget && now_us < g_flash_until;
    gfx_fill(d->x, d->y, d->w, d->h, GFX_PANEL);
    gfx_frame(d->x, d->y, d->w, d->h, flash ? GFX_TEXT : GFX_FRAME);
    gfx_text(d->x + 6, d->y + 2, GFX_DIM, d->title, d->w - 12);
    if (d->kind == PSPC_W_CLOCK) {
        draw_clock(d);
        return;
    }
    if (!g_has_value[i] || v->kind != d->kind) {
        gfx_text(d->x + 8, d->y + 24, GFX_DIM, "...", 0);
        return;
    }
    switch (d->kind) {
    case PSPC_W_SYSTEM: draw_system(d, &v->u.system); break;
    case PSPC_W_NOW_PLAYING: draw_now_playing(d, &v->u.now); break;
    case PSPC_W_NOTIFICATIONS: draw_notifications(d, &v->u.notes); break;
    case PSPC_W_CALENDAR: draw_calendar(d, &v->u.calendar); break;
    case PSPC_W_CLAUDE_CODE: draw_claude(d, &v->u.claude); break;
    default: break;
    }
}

void widgets_draw(uint64_t now_us)
{
    char head[64];
    int i;
    gfx_fill(0, 0, UI_WIDTH, UI_HEIGHT, GFX_BG);
    if (!g_have_layout) {
        gfx_text(16, 120, GFX_DIM, "Waiting for the widget layout from the PC...", 0);
        return;
    }
    if (macropad_is_open()) {
        macropad_draw(now_us);
        cursor_draw();
        return;
    }
    snprintf(head, sizeof head, "%s", g_layout.pages[g_page]);
    gfx_text(8, 2, GFX_ACCENT, head, 200);
    snprintf(head, sizeof head, "%d/%d  L/R page  \xE2\x96\xB3 macros  START menu", g_page + 1, g_layout.page_count);
    gfx_text(UI_WIDTH - 8 - font_text_width(head), 2, GFX_DIM, head, 0);
    for (i = 0; i < g_layout.widget_count; i++)
        if (g_layout.widgets[i].page == g_page) draw_widget(i, now_us);
    cursor_draw();
}
