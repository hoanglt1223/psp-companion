#include "mp3_player.h"

#include <pspaudiocodec.h>
#include <pspkernel.h>
#include <psputility.h>
#include <psputility_avmodules.h>
#include <string.h>

#include "jitter.h"

#define MODULE_ALREADY_LOADED 0x80020139

/* The codec context (SceAudiocodecCodec, see PPSSPP Core/HLE/sceAudiocodec.h): word 6 is the input
 * frame, 7 the bytes it read, 8 the output buffer, 9 the bytes it wrote, 10 an upper bound of the
 * frame size used for the cache writeback. */
enum { CTX_IN = 6, CTX_IN_READ = 7, CTX_OUT = 8, CTX_OUT_WRITTEN = 9, CTX_MAX_FRAME = 10 };

static unsigned long g_ctx[65] __attribute__((aligned(64)));
static uint8_t g_in[(JB_MAX_FRAME + 63) & ~63] __attribute__((aligned(64)));
static int g_ready;

int mp3p_init(void)
{
    int rc;
    if (g_ready) return 0;
    rc = sceUtilityLoadAvModule(PSP_AV_MODULE_AVCODEC);
    if (rc < 0 && (unsigned)rc != MODULE_ALREADY_LOADED) return rc;
    memset(g_ctx, 0, sizeof g_ctx);
    if ((rc = sceAudiocodecCheckNeedMem(g_ctx, PSP_CODEC_MP3)) < 0) return rc;
    if ((rc = sceAudiocodecGetEDRAM(g_ctx, PSP_CODEC_MP3)) < 0) return rc;
    if ((rc = sceAudiocodecInit(g_ctx, PSP_CODEC_MP3)) < 0) {
        sceAudiocodecReleaseEDRAM(g_ctx);
        return rc;
    }
    g_ready = 1;
    return 0;
}

int mp3p_decode(const uint8_t *frame, size_t len, int16_t *pcm)
{
    int rc;
    if (!g_ready || len == 0 || len > JB_MAX_FRAME || mp3_frame_len(frame, len) != len) return -1;
    memcpy(g_in, frame, len);
    /* The Media Engine reads and writes main memory directly, bypassing the CPU cache. */
    sceKernelDcacheWritebackRange(g_in, sizeof g_in);
    sceKernelDcacheWritebackInvalidateRange(pcm, MP3P_SAMPLES * 4);
    g_ctx[CTX_IN] = (unsigned long)g_in;
    g_ctx[CTX_IN_READ] = len;
    g_ctx[CTX_OUT] = (unsigned long)pcm;
    g_ctx[CTX_OUT_WRITTEN] = MP3P_SAMPLES * 4;
    g_ctx[CTX_MAX_FRAME] = len;
    rc = sceAudiocodecDecode(g_ctx, PSP_CODEC_MP3);
    sceKernelDcacheInvalidateRange(pcm, MP3P_SAMPLES * 4);
    if (rc < 0) return rc;
    return g_ctx[CTX_OUT_WRITTEN] == MP3P_SAMPLES * 4 ? 0 : -1;
}

void mp3p_term(void)
{
    if (!g_ready) return;
    sceAudiocodecReleaseEDRAM(g_ctx);
    g_ready = 0;
}
