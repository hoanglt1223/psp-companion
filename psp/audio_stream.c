#include "audio_stream.h"

#include <pspaudio.h>
#include <pspkernel.h>
#include <string.h>

#include "jitter.h"
#include "mp3_player.h"
#include "net.h"
#include "protocol.h"

#define OUTPUT_RATE 44100

static jitter_buf g_jb;
static SceUID g_lock = -1;
static volatile int g_active;
static uint8_t g_stream_id, g_kind;
static int g_speaker_wanted;
static int g_volume_pct = 80;
static int g_decoder_error;
static int g_reserved;

static int16_t g_pcm[MP3P_SAMPLES * 2] __attribute__((aligned(64)));
static int16_t g_last[MP3P_SAMPLES * 2] __attribute__((aligned(64)));
static int16_t g_silence[MP3P_SAMPLES * 2] __attribute__((aligned(64)));
static uint8_t g_frame[JB_MAX_FRAME];

static void lock(void)
{
    sceKernelWaitSema(g_lock, 1, NULL);
}

static void unlock(void)
{
    sceKernelSignalSema(g_lock, 1);
}

static int player_thread(SceSize args, void *argp)
{
    int concealed = 0;
    (void)args;
    (void)argp;
    for (;;) {
        const uint8_t *data;
        size_t len = 0;
        int r;
        const int16_t *out = g_silence;
        if (!g_active) {
            if (g_reserved) {
                sceAudioSRCChRelease();
                g_reserved = 0;
            }
            sceKernelDelayThread(20 * 1000);
            continue;
        }
        if (!g_reserved) {
            if (sceAudioSRCChReserve(MP3P_SAMPLES, OUTPUT_RATE, 2) < 0) {
                sceKernelDelayThread(100 * 1000);
                continue;
            }
            g_reserved = 1;
        }
        lock();
        r = jb_next(&g_jb, &data, &len);
        if (r == JB_FRAME) memcpy(g_frame, data, len);
        unlock();
        if (r == JB_FRAME && mp3p_decode(g_frame, len, g_pcm) == 0) {
            memcpy(g_last, g_pcm, sizeof g_last);
            concealed = 0;
            out = g_pcm;
        } else if (r == JB_LOST && concealed++ == 0) {
            /* Repeat the previous frame once at half volume; further losses play silence. */
            int i;
            for (i = 0; i < MP3P_SAMPLES * 2; i++) g_pcm[i] = (int16_t)(g_last[i] / 2);
            out = g_pcm;
        }
        /* Blocks for about one frame (26 ms), which paces the loop. */
        sceAudioSRCOutputBlocking(PSP_AUDIO_VOLUME_MAX * g_volume_pct / 100, (void *)out);
    }
    return 0;
}

int audio_init(void)
{
    int th;
    jb_reset(&g_jb);
    g_lock = sceKernelCreateSema("audio_jb", 0, 1, 1, NULL);
    if (g_lock < 0) return g_lock;
    g_decoder_error = mp3p_init();
    /* Above the UI thread so rendering never starves the audio output. */
    th = sceKernelCreateThread("audio_player", player_thread, 0x12, 0x4000, PSP_THREAD_ATTR_USER, NULL);
    if (th < 0) return th;
    return sceKernelStartThread(th, 0, NULL);
}

static void stop_locked(void)
{
    g_active = 0;
    jb_reset(&g_jb);
}

void audio_reset(void)
{
    if (g_lock < 0) return;
    lock();
    stop_locked();
    unlock();
    g_speaker_wanted = 0;
}

int audio_on_message(const uint8_t *msg, size_t len)
{
    pspc_stream_start s;
    uint8_t id, reason;
    switch (msg[0]) {
    case PSPC_MSG_AUDIO_STREAM_START:
        if (pspc_parse_stream_start(msg, len, &s) != PSPC_OK) return 1;
        lock();
        stop_locked();
        /* Only what the Media Engine path decodes: MP3 at 44.1 kHz. */
        if (s.codec == PSPC_AUDIO_CODEC_MP3 && s.sample_rate == OUTPUT_RATE && g_decoder_error == 0) {
            g_stream_id = s.stream_id;
            g_kind = s.kind;
            g_active = 1;
        }
        unlock();
        return 1;
    case PSPC_MSG_AUDIO_STREAM_STOP:
        if (pspc_parse_stream_stop(msg, len, &id, &reason) != PSPC_OK) return 1;
        lock();
        if (g_active && id == g_stream_id) stop_locked();
        unlock();
        return 1;
    default:
        return 0;
    }
}

void audio_on_datagram(const uint8_t *msg, size_t len)
{
    uint8_t id;
    uint32_t seq, pts;
    const uint8_t *data;
    size_t n;
    if (pspc_parse_audio_packet(msg, len, &id, &seq, &pts, &data, &n) != PSPC_OK) return;
    lock();
    if (g_active && id == g_stream_id) jb_put(&g_jb, seq, pts, data, n);
    unlock();
}

void audio_set_speaker(int on)
{
    uint8_t m[4];
    if (net_send(m, pspc_build_stream_request(m, PSPC_AUDIO_KIND_SPEAKER, (uint8_t)(on != 0))) == 0)
        g_speaker_wanted = on != 0;
}

void audio_stop_current(void)
{
    uint8_t m[4], id;
    int was_active, was_speaker;
    lock();
    was_active = g_active;
    was_speaker = g_kind == PSPC_AUDIO_KIND_SPEAKER;
    id = g_stream_id;
    if (was_active) stop_locked();
    unlock();
    if (!was_active) return;
    net_send(m, pspc_build_stream_stop(m, id, PSPC_STOP_USER));
    if (was_speaker) g_speaker_wanted = 0;
}

void audio_volume_step(int delta_pct)
{
    int v = g_volume_pct + delta_pct;
    g_volume_pct = v < 0 ? 0 : v > 100 ? 100 : v;
}

void audio_get_status(audio_status *out)
{
    memset(out, 0, sizeof *out);
    lock();
    out->active = g_active;
    out->kind = g_kind;
    out->depth_ms = jb_depth_ms(&g_jb);
    out->target_ms = g_jb.target_ms;
    out->played = g_jb.played;
    out->lost = g_jb.lost;
    out->late = g_jb.late;
    out->underruns = g_jb.underruns;
    unlock();
    out->speaker_wanted = g_speaker_wanted;
    out->decoder_error = g_decoder_error;
    out->volume_pct = g_volume_pct;
}
