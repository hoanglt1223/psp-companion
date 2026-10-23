#include "jitter.h"

#include <string.h>

/* Grow the target on a network underrun; shrink it slowly after 30 s without one. */
#define GROW_MS 20
#define SHRINK_MS 10
#define STABLE_FRAMES 1150

size_t mp3_frame_len(const uint8_t *p, size_t avail)
{
    static const uint16_t kbps[16] = {0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0};
    static const uint32_t rates[4] = {44100, 48000, 32000, 0};
    uint32_t k, r;
    if (avail < 4 || p[0] != 0xFF || (p[1] & 0xFE) != 0xFA) return 0;
    k = kbps[p[2] >> 4];
    r = rates[(p[2] >> 2) & 3];
    if (k == 0 || r == 0) return 0;
    return (size_t)(144u * k * 1000u / r) + ((p[2] >> 1) & 1);
}

static int32_t diff(uint32_t a, uint32_t b)
{
    return (int32_t)(a - b);
}

void jb_reset(jitter_buf *jb)
{
    int i;
    for (i = 0; i < JB_SLOTS; i++) jb->slots[i].used = 0;
    jb->buffering = 1;
    jb->have_next = 0;
    jb->starved = 0;
    jb->next_seq = 0;
    jb->last_pts = 0;
    jb->target_ms = JB_START_MS;
    jb->stable_frames = 0;
    jb->played = jb->lost = jb->late = jb->dropped = jb->underruns = 0;
}

int jb_put(jitter_buf *jb, uint32_t seq, uint32_t pts_ms, const uint8_t *frame, size_t len)
{
    jb_slot *s = &jb->slots[seq % JB_SLOTS];
    if (len == 0 || len > JB_MAX_FRAME || mp3_frame_len(frame, len) != len) {
        jb->dropped++;
        return -1;
    }
    if (jb->have_next && diff(seq, jb->next_seq) < 0) {
        jb->late++;
        return -1;
    }
    if (jb->have_next && diff(seq, jb->next_seq) >= JB_SLOTS) {
        jb->dropped++;
        return -1;
    }
    if (s->used && s->seq == seq) return -1;
    if (s->used && jb->have_next && diff(s->seq, jb->next_seq) >= 0) {
        jb->dropped++; /* the window is full of frames still to play */
        return -1;
    }
    if (jb->starved && jb->have_next && seq == jb->next_seq) {
        /* Consecutive seq with a pts jump means the host skipped silence; otherwise the network was late. */
        if (diff(pts_ms, jb->last_pts) <= 2 * (JB_FRAME_US / 1000) + 1) {
            if (jb->target_ms + GROW_MS <= JB_MAX_MS) jb->target_ms += GROW_MS;
            jb->underruns++;
            jb->stable_frames = 0;
        }
        jb->starved = 0;
    }
    s->seq = seq;
    s->pts_ms = pts_ms;
    s->len = (uint16_t)len;
    s->used = 1;
    memcpy(s->data, frame, len);
    return 0;
}

static int ahead(const jitter_buf *jb, const jb_slot *s)
{
    return s->used && (!jb->have_next || diff(s->seq, jb->next_seq) >= 0);
}

uint32_t jb_depth_ms(const jitter_buf *jb)
{
    int i, n = 0;
    for (i = 0; i < JB_SLOTS; i++)
        if (ahead(jb, &jb->slots[i])) n++;
    return (uint32_t)n * JB_FRAME_US / 1000;
}

static int lowest_seq(const jitter_buf *jb, uint32_t *out)
{
    int i, found = 0;
    for (i = 0; i < JB_SLOTS; i++) {
        const jb_slot *s = &jb->slots[i];
        if (!ahead(jb, s)) continue;
        if (!found || diff(s->seq, *out) < 0) *out = s->seq;
        found = 1;
    }
    return found;
}

int jb_next(jitter_buf *jb, const uint8_t **data, size_t *len)
{
    jb_slot *s;
    uint32_t lowest;
    if (jb->buffering) {
        if (jb_depth_ms(jb) < jb->target_ms) return JB_SILENCE;
        jb->buffering = 0;
        jb->starved = 0;
        if (!jb->have_next && lowest_seq(jb, &lowest)) {
            jb->next_seq = lowest;
            jb->have_next = 1;
        }
    }
    s = &jb->slots[jb->next_seq % JB_SLOTS];
    if (s->used && s->seq == jb->next_seq) {
        s->used = 0;
        jb->next_seq++;
        jb->last_pts = s->pts_ms;
        jb->played++;
        if (++jb->stable_frames >= STABLE_FRAMES) {
            jb->stable_frames = 0;
            if (jb->target_ms >= JB_MIN_MS + SHRINK_MS) jb->target_ms -= SHRINK_MS;
        }
        *data = s->data;
        *len = s->len;
        return JB_FRAME;
    }
    if (lowest_seq(jb, &lowest)) {
        /* Later frames are here, so this one is too late to play. */
        jb->next_seq++;
        jb->last_pts += JB_FRAME_US / 1000;
        jb->lost++;
        return JB_LOST;
    }
    jb->buffering = 1;
    jb->starved = 1;
    return JB_SILENCE;
}
