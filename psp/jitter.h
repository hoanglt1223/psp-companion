/* Jitter buffer for one MP3 audio stream (one frame per packet, protocol/PROTOCOL.md "Audio streams").
 * Pure C99 (no PSP headers), so the host test harness can drive it with simulated arrival patterns.
 * Not thread-safe: the caller serialises jb_put (network) and jb_next (player). */
#ifndef PSPC_JITTER_H
#define PSPC_JITTER_H

#include <stddef.h>
#include <stdint.h>

#define JB_SLOTS 64
#define JB_MAX_FRAME 1441 /* largest MPEG-1 Layer III frame (320 kbps at 32 kHz, padded) */
#define JB_FRAME_US 26122 /* 1152 samples at 44.1 kHz */
#define JB_MIN_MS 150
#define JB_MAX_MS 250
#define JB_START_MS 180

enum jb_result { JB_FRAME = 0, JB_LOST = 1, JB_SILENCE = 2 };

typedef struct {
    uint32_t seq, pts_ms;
    uint16_t len;
    uint8_t used;
    uint8_t data[JB_MAX_FRAME];
} jb_slot;

typedef struct {
    jb_slot slots[JB_SLOTS];
    int buffering;   /* 1 until the depth reaches the target */
    int have_next;   /* next_seq is known */
    int starved;     /* ran dry; judged when packets resume */
    uint32_t next_seq;
    uint32_t last_pts; /* pts of the last frame played */
    uint32_t target_ms;
    uint32_t stable_frames; /* played since the last network underrun */
    /* statistics */
    uint32_t played, lost, late, dropped, underruns;
} jitter_buf;

/* Length of the MPEG-1 Layer III frame at p, or 0 if p is not one. */
size_t mp3_frame_len(const uint8_t *p, size_t avail);

void jb_reset(jitter_buf *jb);
/* Stores one packet's frame. Returns 0 if kept, -1 if dropped (duplicate, late, too far ahead, not a frame). */
int jb_put(jitter_buf *jb, uint32_t seq, uint32_t pts_ms, const uint8_t *frame, size_t len);
/* Called once per frame time by the player. On JB_FRAME, *data and *len point into the buffer and stay
 * valid until the next jb_put of the same slot (the caller copies before releasing its lock). */
int jb_next(jitter_buf *jb, const uint8_t **data, size_t *len);
/* Buffered audio ahead of the play position. */
uint32_t jb_depth_ms(const jitter_buf *jb);

#endif
