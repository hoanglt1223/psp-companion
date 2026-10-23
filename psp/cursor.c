#include "cursor.h"

#include "gfx.h"

#define DEAD_ZONE 28
#define MAX_SPEED 9.0f /* pixels per frame at full tilt */
#define HIDE_AFTER_US (4 * 1000 * 1000ULL)

static float g_x = UI_WIDTH / 2, g_y = UI_HEIGHT / 2;
static uint64_t g_last_move;
static int g_visible;

void cursor_reset(void)
{
    g_x = UI_WIDTH / 2;
    g_y = UI_HEIGHT / 2;
    g_visible = 0;
}

/* Quadratic response: fine control near the centre, fast travel at the edge. */
static float axis(int raw)
{
    int d = raw - 128, mag = d < 0 ? -d : d;
    float t;
    if (mag <= DEAD_ZONE) return 0.0f;
    t = (float)(mag - DEAD_ZONE) / (float)(128 - DEAD_ZONE);
    if (t > 1.0f) t = 1.0f;
    t = 0.4f + (MAX_SPEED - 0.4f) * t * t;
    return d < 0 ? -t : t;
}

void cursor_update(const ui_pad *pad, uint64_t now_us)
{
    float dx = axis(pad->lx), dy = axis(pad->ly);
    if (dx != 0.0f || dy != 0.0f) {
        g_x += dx;
        g_y += dy;
        if (g_x < 0) g_x = 0;
        if (g_y < 0) g_y = 0;
        if (g_x > UI_WIDTH - 1) g_x = UI_WIDTH - 1;
        if (g_y > UI_HEIGHT - 1) g_y = UI_HEIGHT - 1;
        g_last_move = now_us;
        g_visible = 1;
    } else if (g_visible && now_us - g_last_move > HIDE_AFTER_US) {
        g_visible = 0;
    }
}

int cursor_visible(void)
{
    return g_visible;
}

int cursor_x(void)
{
    return (int)g_x;
}

int cursor_y(void)
{
    return (int)g_y;
}

void cursor_draw(void)
{
    int x = (int)g_x, y = (int)g_y, i;
    if (!g_visible) return;
    /* An arrow: a dark outline under a white fill keeps it visible on any widget. */
    for (i = 0; i < 12; i++) {
        gfx_fill(x - 1, y + i - 1, i / 2 + 3, 3, 0xFF000000);
    }
    for (i = 0; i < 11; i++) {
        gfx_fill(x, y + i, i / 2 + 1, 1, GFX_TEXT);
    }
}
