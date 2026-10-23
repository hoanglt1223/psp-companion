/* Flattens the PSP's parsed C structs into text so the Rust tests need no struct layout mirrors. */
#include <stdarg.h>
#include <stdio.h>
#include <string.h>

#include "protocol.h"

typedef struct {
    char *p;
    size_t cap, n;
} sbuf;

static void put(sbuf *b, const char *fmt, ...)
{
    va_list ap;
    int k;
    if (b->n >= b->cap) return;
    va_start(ap, fmt);
    k = vsnprintf(b->p + b->n, b->cap - b->n, fmt, ap);
    va_end(ap);
    if (k > 0) b->n += (size_t)k;
}

int xc_layout(const uint8_t *data, size_t len, char *out, size_t cap)
{
    static pspc_layout l;
    sbuf b = {out, cap, 0};
    int i, rc = pspc_parse_layout(data, len, &l);
    out[0] = 0;
    if (rc != PSPC_OK) return rc;
    put(&b, "pages");
    for (i = 0; i < l.page_count; i++) put(&b, "|%s", l.pages[i]);
    for (i = 0; i < l.widget_count; i++) {
        const pspc_widget_def *d = &l.widgets[i];
        put(&b, "\nw %u %u %u %u %u %u %u %u %s", d->id, d->kind, d->page, d->x, d->y, d->w, d->h, d->flags, d->title);
    }
    for (i = 0; i < l.macro_count; i++) put(&b, "\nm %u %s", l.macros[i].slot, l.macros[i].label);
    return 0;
}

int xc_widget(uint8_t kind, const uint8_t *data, size_t len, char *out, size_t cap)
{
    static pspc_widget_value v;
    sbuf b = {out, cap, 0};
    int i, rc = pspc_parse_widget_value(kind, data, len, &v);
    out[0] = 0;
    if (rc != PSPC_OK) return rc;
    switch (kind) {
    case PSPC_W_SYSTEM:
        put(&b, "%u %u %u %u %d %d", v.u.system.cpu_pct, v.u.system.gpu_pct, (unsigned)v.u.system.ram_used_mb,
            (unsigned)v.u.system.ram_total_mb, v.u.system.cpu_temp, v.u.system.gpu_temp);
        break;
    case PSPC_W_NOW_PLAYING:
        put(&b, "%u %llx %s|%s", v.u.now.state, (unsigned long long)v.u.now.cover, v.u.now.title, v.u.now.artist);
        break;
    case PSPC_W_NOTIFICATIONS:
        put(&b, "%u", v.u.notes.count);
        for (i = 0; i < v.u.notes.count; i++)
            put(&b, "\n%s|%s|%s|%s", v.u.notes.items[i].app, v.u.notes.items[i].title, v.u.notes.items[i].body,
                v.u.notes.items[i].time);
        break;
    case PSPC_W_CALENDAR:
        put(&b, "%u %s|%s", v.u.calendar.has_event, v.u.calendar.title, v.u.calendar.when);
        break;
    case PSPC_W_CLAUDE_CODE:
        put(&b, "%u", v.u.claude.count);
        for (i = 0; i < v.u.claude.count; i++)
            put(&b, "\n%s|%u|%s", v.u.claude.items[i].project, v.u.claude.items[i].state, v.u.claude.items[i].detail);
        break;
    default:
        break;
    }
    return 0;
}

/* ---------- jitter buffer ---------- */
#include <stdlib.h>

#include "jitter.h"

jitter_buf *xc_jb_new(void)
{
    jitter_buf *jb = (jitter_buf *)malloc(sizeof *jb);
    if (jb) jb_reset(jb);
    return jb;
}

void xc_jb_free(jitter_buf *jb)
{
    free(jb);
}

/* played, lost, late, dropped, underruns, target_ms, buffering */
void xc_jb_stats(const jitter_buf *jb, uint32_t out[7])
{
    out[0] = jb->played;
    out[1] = jb->lost;
    out[2] = jb->late;
    out[3] = jb->dropped;
    out[4] = jb->underruns;
    out[5] = jb->target_ms;
    out[6] = (uint32_t)jb->buffering;
}
