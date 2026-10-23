#include "ui.h"

#include <pspctrl.h>
#include <pspdebug.h>
#include <pspdisplay.h>
#include <pspgu.h>
#include <pspkernel.h>
#include <psputility.h>
#include <stdarg.h>
#include <stdio.h>
#include <string.h>

#include "rng.h"

#define BUF_WIDTH 512
#define SCR_HEIGHT 272
#define FRAME_SIZE (BUF_WIDTH * SCR_HEIGHT * 4)
#define BG_COLOR 0xFF302418

static unsigned int __attribute__((aligned(16))) g_list[16 * 1024];
static void *g_draw = (void *)0;
static uint32_t g_prev_buttons;

void ui_init(void)
{
    sceGuInit();
    sceGuStart(GU_DIRECT, g_list);
    sceGuDrawBuffer(GU_PSM_8888, (void *)0, BUF_WIDTH);
    sceGuDispBuffer(480, SCR_HEIGHT, (void *)FRAME_SIZE, BUF_WIDTH);
    sceGuOffset(2048 - 240, 2048 - 136);
    sceGuViewport(2048, 2048, 480, SCR_HEIGHT);
    sceGuScissor(0, 0, 480, SCR_HEIGHT);
    sceGuEnable(GU_SCISSOR_TEST);
    sceGuFinish();
    sceGuSync(GU_SYNC_FINISH, GU_SYNC_WHAT_DONE);
    sceDisplayWaitVblankStart();
    sceGuDisplay(GU_TRUE);

    pspDebugScreenInitEx(NULL, PSP_DISPLAY_PIXEL_FORMAT_8888, 0);
    pspDebugScreenEnableBackColor(0);

    sceCtrlSetSamplingCycle(0);
    sceCtrlSetSamplingMode(PSP_CTRL_MODE_DIGITAL);
}

static void clear_frame(void)
{
    sceGuStart(GU_DIRECT, g_list);
    sceGuClearColor(BG_COLOR);
    sceGuClear(GU_COLOR_BUFFER_BIT);
    sceGuFinish();
    sceGuSync(GU_SYNC_FINISH, GU_SYNC_WHAT_DONE);
}

void ui_begin(void)
{
    clear_frame();
    /* Text is written by the CPU into the draw buffer after the GE has finished clearing it. */
    pspDebugScreenSetOffset((int)(uintptr_t)g_draw);
}

void ui_end(void)
{
    sceDisplayWaitVblankStart();
    g_draw = sceGuSwapBuffers();
}

void ui_text(int col, int row, uint32_t color, const char *fmt, ...)
{
    char line[UI_COLS + 1];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(line, sizeof line, fmt, ap);
    va_end(ap);
    pspDebugScreenSetTextColor(color);
    pspDebugScreenSetXY(col, row);
    pspDebugScreenPuts(line);
}

void ui_dialog_frame_begin(void)
{
    clear_frame();
}

void ui_dialog_frame_end(void)
{
    sceDisplayWaitVblankStart();
    g_draw = sceGuSwapBuffers();
}

uint32_t ui_poll_pressed(void)
{
    SceCtrlData pad;
    uint32_t pressed;
    sceCtrlPeekBufferPositive(&pad, 1);
    pressed = pad.Buttons & ~g_prev_buttons;
    g_prev_buttons = pad.Buttons;
    if (pressed) rng_mix(&pad, sizeof pad);
    return pressed;
}

static void utf8_to_utf16(const char *in, unsigned short *out, size_t cap)
{
    size_t i = 0;
    while (*in && i + 1 < cap) out[i++] = (unsigned char)*in++; /* inputs here are ASCII */
    out[i] = 0;
}

int ui_osk(const char *title, const char *initial, int max_len, char *out, size_t out_cap)
{
    static unsigned short desc[64], intext[64], outtext[64];
    SceUtilityOskData data;
    SceUtilityOskParams params;
    int done = 0, i, n = 0;

    if (max_len > 63) max_len = 63;
    utf8_to_utf16(title, desc, 64);
    utf8_to_utf16(initial, intext, 64);
    memset(outtext, 0, sizeof outtext);

    memset(&data, 0, sizeof data);
    data.language = PSP_UTILITY_OSK_LANGUAGE_DEFAULT;
    data.lines = 1;
    data.unk_24 = 1;
    data.inputtype = PSP_UTILITY_OSK_INPUTTYPE_ALL;
    data.desc = desc;
    data.intext = intext;
    data.outtextlength = 64;
    data.outtextlimit = max_len;
    data.outtext = outtext;

    memset(&params, 0, sizeof params);
    params.base.size = sizeof params;
    sceUtilityGetSystemParamInt(PSP_SYSTEMPARAM_ID_INT_LANGUAGE, &params.base.language);
    sceUtilityGetSystemParamInt(PSP_SYSTEMPARAM_ID_INT_BUTTON_SWAP, &params.base.buttonSwap);
    params.base.graphicsThread = 17;
    params.base.accessThread = 19;
    params.base.fontThread = 18;
    params.base.soundThread = 16;
    params.datacount = 1;
    params.data = &data;

    if (sceUtilityOskInitStart(&params) < 0) return -1;
    while (!done) {
        ui_dialog_frame_begin();
        switch (sceUtilityOskGetStatus()) {
        case PSP_UTILITY_DIALOG_VISIBLE:
            sceUtilityOskUpdate(1);
            break;
        case PSP_UTILITY_DIALOG_QUIT:
            sceUtilityOskShutdownStart();
            break;
        case PSP_UTILITY_DIALOG_NONE:
            done = 1;
            break;
        default:
            break;
        }
        ui_dialog_frame_end();
    }
    rng_mix(outtext, sizeof outtext);
    if (data.result == PSP_UTILITY_OSK_RESULT_CANCELLED) return -1;
    for (i = 0; outtext[i] && (size_t)n + 1 < out_cap; i++) {
        if (outtext[i] < 0x80) out[n++] = (char)outtext[i];
    }
    out[n] = '\0';
    return 0;
}
