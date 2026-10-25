#include "power.h"

#include <pspkernel.h>
#include <psppower.h>
#include <psputility.h>
#include <stdio.h>

#include "font.h"
#include "gfx.h"

#define CPU_FULL 222
#define BUS_FULL 111
/* Enough for the UI and the network; audio and recording always run at full speed. */
#define CPU_SAVER 133
#define BUS_SAVER 66
#define SAVER_AFTER_US (15 * 1000 * 1000ULL)
#define BATTERY_POLL_US (10 * 1000 * 1000ULL)

static volatile int g_resumed;
static uint64_t g_last_activity, g_last_battery;
static int g_saving_clock = -1;
static power_status g_status = {-1, -1, 0, 0, 0, CPU_FULL};

static int power_callback(int unknown, int flags, void *common)
{
    (void)unknown;
    (void)common;
    if (flags & PSP_POWER_CB_RESUME_COMPLETE) g_resumed = 1;
    return 0;
}

void power_init(void)
{
    int cbid = sceKernelCreateCallback("Power Callback", power_callback, NULL);
    if (cbid < 0) return;
    /* Slot -1 lets the system pick a free slot on firmware 6.xx; older firmware needs a fixed slot. */
    if (scePowerRegisterCallback(-1, cbid) < 0) scePowerRegisterCallback(0, cbid);
}

static void set_clock(int saver)
{
    if (saver == g_saving_clock) return;
    if (saver)
        scePowerSetClockFrequency(CPU_SAVER, CPU_SAVER, BUS_SAVER);
    else
        scePowerSetClockFrequency(CPU_FULL, CPU_FULL, BUS_FULL);
    g_saving_clock = saver;
    g_status.cpu_mhz = scePowerGetCpuClockFrequencyInt();
}

void power_full_speed(void)
{
    set_clock(0);
}

static void read_battery(void)
{
    int pct = scePowerIsBatteryExist() ? scePowerGetBatteryLifePercent() : -1;
    int mins = scePowerGetBatteryLifeTime();
    g_status.percent = pct >= 0 && pct <= 100 ? pct : -1;
    g_status.charging = scePowerIsBatteryCharging() > 0;
    g_status.on_ac = scePowerIsPowerOnline() > 0;
    g_status.minutes = !g_status.charging && mins > 0 ? mins : -1;
    g_status.low = scePowerIsLowBattery() > 0;
}

void power_tick(uint64_t now_us, int input, int busy, int cpu_saver, int keep_screen_on)
{
    if (g_last_activity == 0 || input || busy) g_last_activity = now_us;
    set_clock(cpu_saver && !busy && now_us - g_last_activity > SAVER_AFTER_US);

    /* Auto-sleep would drop the connection, so only the backlight is left to the idle timer. */
    scePowerTick(PSP_POWER_TICK_SUSPEND);
    if (keep_screen_on || busy) scePowerTick(PSP_POWER_TICK_DISPLAY);

    if (g_last_battery == 0 || now_us - g_last_battery > BATTERY_POLL_US) {
        g_last_battery = now_us;
        read_battery();
    }
}

int power_take_resumed(void)
{
    if (!g_resumed) return 0;
    g_resumed = 0;
    g_last_battery = 0;
    /* The clock setting does not survive sleep on every firmware. */
    g_saving_clock = -1;
    set_clock(0);
    return 1;
}

void power_get(power_status *out)
{
    if (g_status.cpu_mhz <= 0) g_status.cpu_mhz = scePowerGetCpuClockFrequencyInt();
    *out = g_status;
}

int power_wlan_saving(void)
{
    int v = 0;
    if (sceUtilityGetSystemParamInt(PSP_SYSTEMPARAM_ID_INT_WLAN_POWERSAVE, &v) < 0) return -1;
    return v == PSP_SYSTEMPARAM_WLAN_POWERSAVE_ON;
}

int power_set_wlan_saving(int on)
{
    return sceUtilitySetSystemParamInt(PSP_SYSTEMPARAM_ID_INT_WLAN_POWERSAVE,
                                       on ? PSP_SYSTEMPARAM_WLAN_POWERSAVE_ON : PSP_SYSTEMPARAM_WLAN_POWERSAVE_OFF);
}

void power_draw_badge(int right_x, int y)
{
    char text[12];
    uint32_t color;
    int tw, x, fill;
    const power_status *s = &g_status;
    if (s->percent < 0) {
        if (!s->on_ac) return;
        snprintf(text, sizeof text, "AC");
    } else {
        snprintf(text, sizeof text, "%d%%", s->percent);
    }
    color = s->charging || s->on_ac ? GFX_ACCENT : s->low || s->percent <= 15 ? GFX_BAD
                                                  : s->percent <= 30          ? GFX_WARN
                                                                              : GFX_GOOD;
    tw = font_text_width(text);
    x = right_x - tw;
    gfx_text(x, y, color, text, 0);
    /* Battery outline 18x9 with a 2 px tip, vertically centred on the text. */
    x -= 24;
    gfx_frame(x, y + 4, 18, 9, GFX_DIM);
    gfx_fill(x + 18, y + 6, 2, 5, GFX_DIM);
    fill = s->percent < 0 ? 14 : (s->percent * 14 + 50) / 100;
    if (fill > 0) gfx_fill(x + 2, y + 6, fill, 5, color);
}
