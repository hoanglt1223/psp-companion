#include "voice.h"

#include <pspaudio.h>
#include <pspkernel.h>
#include <string.h>

#include "adpcm.h"
#include "gfx.h"
#include "net.h"
#include "protocol.h"
#include "text_view.h"

#define BLOCK 1024 /* samples per VoiceChunk, about 93 ms */
#define QUEUE 8
#define MIC_GAIN 0x7FFF
#define MAX_BLOCKS ((VOICE_RATE * VOICE_MAX_SECONDS + BLOCK - 1) / BLOCK)

enum phase { IDLE, RECORDING, DRAINING };

static SceUID g_lock = -1;
static volatile int g_capture; /* the thread records while set */
static volatile int g_in_capture; /* the thread is inside sceAudioInputBlocking */
static int16_t g_queue[QUEUE][BLOCK];
static int g_head, g_tail, g_queued;
static volatile int g_level, g_mic_error;

static enum phase g_phase = IDLE;
static uint32_t g_utterance, g_seq, g_blocks;
static uint8_t g_end_reason;
static adpcm_state g_adpcm;

static void lock(void)
{
    sceKernelWaitSema(g_lock, 1, NULL);
}

static void unlock(void)
{
    sceKernelSignalSema(g_lock, 1);
}

static int capture_thread(SceSize args, void *argp)
{
    static int16_t block[BLOCK] __attribute__((aligned(64)));
    (void)args;
    (void)argp;
    for (;;) {
        int i, peak = 0;
        if (!g_capture) {
            sceKernelDelayThread(10 * 1000);
            continue;
        }
        g_in_capture = 1;
        if (sceAudioInputBlocking(BLOCK, VOICE_RATE, block) < 0) {
            g_in_capture = 0;
            g_mic_error = 1;
            g_capture = 0;
            continue;
        }
        g_in_capture = 0;
        for (i = 0; i < BLOCK; i++) {
            int v = block[i] < 0 ? -block[i] : block[i];
            if (v > peak) peak = v;
        }
        g_level = peak * 100 / 32768;
        lock();
        if (g_queued < QUEUE) {
            memcpy(g_queue[g_tail], block, sizeof block);
            g_tail = (g_tail + 1) % QUEUE;
            g_queued++;
        }
        unlock();
    }
    return 0;
}

int voice_init(void)
{
    int th;
    g_lock = sceKernelCreateSema("voice_q", 0, 1, 1, NULL);
    if (g_lock < 0) return g_lock;
    /* The entropy collector may already have initialised input; a repeat call failing is harmless. */
    sceAudioInputInit(0, MIC_GAIN, 0);
    th = sceKernelCreateThread("voice_capture", capture_thread, 0x14, 0x4000, PSP_THREAD_ATTR_USER, NULL);
    if (th < 0) return th;
    return sceKernelStartThread(th, 0, NULL);
}

static void clear_queue(void)
{
    lock();
    g_head = g_tail = g_queued = 0;
    unlock();
}

void voice_reset(void)
{
    g_capture = 0;
    g_phase = IDLE;
    clear_queue();
    tv_reset();
}

void voice_start(void)
{
    uint8_t m[16];
    if (g_phase != IDLE || g_lock < 0) return;
    clear_queue();
    g_utterance++;
    if (net_send(m, pspc_build_voice_start(m, g_utterance, VOICE_RATE)) != 0) {
        tv_set_status("Not connected to the PC.", GFX_BAD);
        return;
    }
    adpcm_init(&g_adpcm);
    g_seq = g_blocks = 0;
    g_mic_error = 0;
    g_phase = RECORDING;
    g_capture = 1;
}

void voice_stop(int cancel)
{
    uint8_t m[8];
    if (g_phase != RECORDING) return;
    g_capture = 0;
    if (cancel) {
        clear_queue();
        g_phase = IDLE;
        net_send(m, pspc_build_voice_end(m, g_utterance, PSPC_VOICE_END_CANCELLED));
        tv_set_status("Cancelled.", GFX_DIM);
        return;
    }
    g_end_reason = PSPC_VOICE_END_RELEASED;
    g_phase = DRAINING;
}

void voice_poll(void)
{
    static uint8_t msg[16 + ADPCM_HEADER + BLOCK / 2];
    uint8_t m[8];
    if (g_phase == IDLE) return;
    if (g_mic_error && g_phase == RECORDING) {
        tv_set_status("The microphone is not available.", GFX_BAD);
        g_end_reason = PSPC_VOICE_END_RELEASED;
        g_phase = DRAINING;
    }
    for (;;) {
        size_t n;
        int have;
        lock();
        have = g_queued > 0;
        unlock();
        if (!have) break;
        n = pspc_build_voice_chunk_header(msg, g_utterance, g_seq);
        /* Encode a copy of the state so a full send queue can retry the same block next frame. */
        {
            adpcm_state st = g_adpcm;
            n += adpcm_encode_chunk(&st, g_queue[g_head], BLOCK, msg + n);
            if (net_send(msg, n) != 0) break;
            g_adpcm = st;
        }
        lock();
        g_head = (g_head + 1) % QUEUE;
        g_queued--;
        unlock();
        g_seq++;
        if (++g_blocks >= MAX_BLOCKS && g_phase == RECORDING) {
            g_capture = 0;
            g_end_reason = PSPC_VOICE_END_TOO_LONG;
            g_phase = DRAINING;
            tv_set_status("60 second limit reached; sending.", GFX_WARN);
        }
    }
    if (g_phase == DRAINING && !g_in_capture) {
        int empty;
        lock();
        empty = g_queued == 0;
        unlock();
        if (empty && net_send(m, pspc_build_voice_end(m, g_utterance, g_end_reason)) == 0) {
            g_phase = IDLE;
            tv_set_status("Thinking...", GFX_DIM);
        }
    }
}

int voice_active(void)
{
    return g_phase == RECORDING;
}

int voice_level(void)
{
    return g_level;
}

int voice_seconds(void)
{
    return (int)(g_blocks * BLOCK / VOICE_RATE);
}

int voice_on_message(const uint8_t *msg, size_t len)
{
    static char text[4096];
    uint8_t type, is_final;
    uint32_t id;
    uint16_t code;
    switch (msg[0]) {
    case PSPC_MSG_VOICE_TRANSCRIPT:
    case PSPC_MSG_VOICE_REPLY_TEXT:
        if (pspc_parse_voice_text(msg, len, &type, &id, &is_final, text, sizeof text) != PSPC_OK) return 1;
        if (type == PSPC_MSG_VOICE_TRANSCRIPT) {
            tv_add(TV_YOU, id, text);
            tv_set_status("Thinking...", GFX_DIM);
        } else {
            if (text[0]) tv_append_reply(id, text);
            tv_set_status(is_final ? "Hold R to ask again. L+R stops speaking." : "Answering...", GFX_DIM);
        }
        return 1;
    case PSPC_MSG_ERROR:
        if (pspc_parse_error(msg, len, &code, text, 200) == PSPC_OK && code != 1) {
            tv_add(TV_NOTICE, 0, text);
            tv_set_status(text, code == 3 ? GFX_WARN : GFX_BAD);
        }
        return 1;
    default:
        return 0;
    }
}
