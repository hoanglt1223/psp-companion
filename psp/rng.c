/* The PSP has no user-mode CSPRNG. Entropy comes from the built-in microphone's
 * noise floor (PSP-3000), timer jitter, and later mixes of input and packet
 * timings; output uses ChaCha20 with fast key erasure. */
#include "rng.h"

#include <pspaudio.h>
#include <pspkernel.h>
#include <psprtc.h>
#include <pspthreadman.h>
#include <string.h>

#include "crypto.h"

#define MIC_SAMPLES 2048

static uint8_t g_key[32];
static uint64_t g_counter;
static int16_t g_mic[MIC_SAMPLES] __attribute__((aligned(64)));
static volatile int g_mic_ok;

static int mic_thread(SceSize args, void *argp)
{
    (void)args;
    (void)argp;
    if (sceAudioInputInit(0, 0x7FFF, 0) < 0) return 0;
    if (sceAudioInputBlocking(MIC_SAMPLES, 44100, g_mic) >= 0) g_mic_ok = 1;
    return 0;
}

/* Capture runs in a thread with a timeout because a missing or busy mic must not hang start-up. */
static int capture_mic(void)
{
    SceUInt timeout = 500 * 1000;
    SceUID th = sceKernelCreateThread("pspc_rng_mic", mic_thread, 0x20, 0x4000, PSP_THREAD_ATTR_USER, NULL);
    if (th < 0) return 0;
    g_mic_ok = 0;
    sceKernelStartThread(th, 0, NULL);
    if (sceKernelWaitThreadEnd(th, &timeout) < 0) {
        sceKernelTerminateDeleteThread(th);
        return 0;
    }
    sceKernelDeleteThread(th);
    return g_mic_ok ? (int)sizeof g_mic : 0;
}

int rng_init(void)
{
    pspc_sha512_ctx s;
    uint8_t h[64];
    u64 tick = 0;
    SceInt64 t = sceKernelGetSystemTimeWide();
    int i, mic;

    pspc_sha512_init(&s);
    pspc_sha512_update(&s, &t, sizeof t);
    sceRtcGetCurrentTick(&tick);
    pspc_sha512_update(&s, &tick, sizeof tick);

    for (i = 0; i < 512; i++) {
        u32 t0 = sceKernelGetSystemTimeLow(), t1;
        volatile u32 spin = (t0 ^ (u32)i) & 0xFF;
        while (spin--) {
        }
        sceKernelDelayThread(0);
        t1 = sceKernelGetSystemTimeLow();
        pspc_sha512_update(&s, &t1, sizeof t1);
    }

    mic = capture_mic();
    if (mic > 0) pspc_sha512_update(&s, g_mic, sizeof g_mic);
    memset(g_mic, 0, sizeof g_mic);

    t = sceKernelGetSystemTimeWide();
    pspc_sha512_update(&s, &t, sizeof t);
    pspc_sha512_final(&s, h);
    memcpy(g_key, h, 32);
    pspc_wipe(h, sizeof h);
    return mic;
}

void rng_mix(const void *data, size_t n)
{
    pspc_sha256_ctx s;
    SceInt64 t = sceKernelGetSystemTimeWide();
    pspc_sha256_init(&s);
    pspc_sha256_update(&s, g_key, 32);
    pspc_sha256_update(&s, &t, sizeof t);
    pspc_sha256_update(&s, data, n);
    pspc_sha256_final(&s, g_key);
}

void rng_bytes(uint8_t *out, size_t n)
{
    uint8_t nonce[12], block[64];
    while (n > 0) {
        size_t take = n < 32 ? n : 32;
        memset(block, 0, sizeof block);
        pspc_nonce(g_counter++, nonce);
        pspc_chacha20_xor(g_key, 0, nonce, block, block, sizeof block);
        memcpy(g_key, block, 32);
        memcpy(out, block + 32, take);
        out += take;
        n -= take;
    }
    pspc_wipe(block, sizeof block);
}
