#include "text_view.h"

#include <stdio.h>
#include <string.h>

#include "font.h"
#include "gfx.h"
#include "ui.h"

#define ENTRIES 8
#define ENTRY_TEXT 2048
#define MAX_LINES 160
#define TEXT_X 12
#define TEXT_W (UI_WIDTH - 2 * TEXT_X)
#define TOP 22
#define BOTTOM (UI_HEIGHT - 40)
#define VISIBLE ((BOTTOM - TOP) / FONT_HEIGHT)

typedef struct {
    int who;
    uint32_t utterance_id;
    char text[ENTRY_TEXT];
} entry;

static entry g_entries[ENTRIES];
static int g_count; /* entries in use, oldest first */
static int g_scroll;
static char g_status[96] = "Hold R and speak. Release to send.";
static uint32_t g_status_color = GFX_DIM;

void tv_reset(void)
{
    g_count = 0;
    g_scroll = 0;
}

static entry *push(int who, uint32_t utterance_id)
{
    entry *e;
    if (g_count == ENTRIES) {
        memmove(&g_entries[0], &g_entries[1], sizeof(entry) * (ENTRIES - 1));
        g_count--;
    }
    e = &g_entries[g_count++];
    e->who = who;
    e->utterance_id = utterance_id;
    e->text[0] = '\0';
    g_scroll = 0;
    return e;
}

static void append(entry *e, const char *s)
{
    size_t have = strlen(e->text), add = strlen(s);
    if (have + add >= ENTRY_TEXT) add = ENTRY_TEXT - 1 - have; /* may split a UTF-8 sequence; the font shows U+FFFD */
    memcpy(e->text + have, s, add);
    e->text[have + add] = '\0';
}

void tv_add(int who, uint32_t utterance_id, const char *text)
{
    append(push(who, utterance_id), text);
}

void tv_append_reply(uint32_t utterance_id, const char *delta)
{
    entry *e = g_count > 0 ? &g_entries[g_count - 1] : NULL;
    if (!e || e->who != TV_CLAUDE || e->utterance_id != utterance_id) e = push(TV_CLAUDE, utterance_id);
    append(e, delta);
    g_scroll = 0;
}

void tv_set_status(const char *status, uint32_t color)
{
    snprintf(g_status, sizeof g_status, "%s", status);
    g_status_color = color;
}

void tv_scroll(int lines)
{
    g_scroll += lines;
    if (g_scroll < 0) g_scroll = 0;
}

typedef struct {
    const char *p;
    size_t len;
    uint32_t color;
} line;

static line g_lines[MAX_LINES];

/* Lays out every entry (newest last), keeping only the last MAX_LINES lines. */
static int layout(void)
{
    int n = 0, i, k;
    for (i = 0; i < g_count; i++) {
        size_t starts[48], lens[48];
        const entry *e = &g_entries[i];
        uint32_t color = e->who == TV_YOU ? GFX_ACCENT : e->who == TV_CLAUDE ? GFX_TEXT : GFX_WARN;
        int got = font_wrap(e->text, TEXT_W, starts, lens, 48);
        for (k = 0; k <= got; k++) {
            if (n == MAX_LINES) {
                memmove(&g_lines[0], &g_lines[1], sizeof(line) * (MAX_LINES - 1));
                n--;
            }
            if (k == got) {
                g_lines[n].p = ""; /* a blank line between entries */
                g_lines[n].len = 0;
            } else {
                g_lines[n].p = e->text + starts[k];
                g_lines[n].len = lens[k];
            }
            g_lines[n].color = color;
            n++;
        }
    }
    return n;
}

void tv_draw(int level, int recording_seconds)
{
    char buf[256];
    int n = layout(), first, i;
    int max_scroll = n > VISIBLE ? n - VISIBLE : 0;
    if (g_scroll > max_scroll) g_scroll = max_scroll;
    first = n - VISIBLE - g_scroll;
    if (first < 0) first = 0;

    gfx_fill(0, 0, UI_WIDTH, UI_HEIGHT, GFX_BG);
    gfx_text(8, 2, GFX_ACCENT, "Assistant", 0);
    gfx_text(150, 2, GFX_DIM, "R talk  L+R stop  UP/DOWN scroll  O back", 0);
    if (g_count == 0) gfx_wrap(TEXT_X, TOP + 8, TEXT_W, 4, GFX_DIM, "Ask anything in Vietnamese or English.");
    for (i = 0; i < VISIBLE && first + i < n; i++) {
        const line *l = &g_lines[first + i];
        size_t len = l->len < sizeof buf - 1 ? l->len : sizeof buf - 1;
        memcpy(buf, l->p, len);
        buf[len] = '\0';
        gfx_text(TEXT_X, TOP + i * FONT_HEIGHT, l->color, buf, TEXT_W);
    }
    if (g_scroll > 0) gfx_text(UI_WIDTH - 90, TOP, GFX_DIM, "\xE2\x86\x91 more", 0);

    gfx_fill(0, BOTTOM + 4, UI_WIDTH, 1, GFX_FRAME);
    if (level >= 0) {
        snprintf(buf, sizeof buf, "Listening... %d s", recording_seconds);
        gfx_text(TEXT_X, BOTTOM + 12, GFX_GOOD, buf, 0);
        gfx_bar(170, BOTTOM + 14, 200, 12, level, level > 90 ? GFX_BAD : GFX_GOOD);
    } else {
        gfx_text(TEXT_X, BOTTOM + 12, g_status_color, g_status, TEXT_W);
    }
}
