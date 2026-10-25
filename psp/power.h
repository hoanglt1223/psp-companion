/* Battery, CPU clock, backlight idle timer, and suspend/resume. */
#ifndef PSPC_POWER_H
#define PSPC_POWER_H

#include <stdint.h>

typedef struct {
    int percent;    /* 0-100, or -1 without a battery */
    int minutes;    /* estimated minutes left, or -1 when unknown or charging */
    int charging;
    int on_ac;
    int low;
    int cpu_mhz;
} power_status;

/* Registers the power callback (needs a thread that sleeps with callbacks, see main.c). */
void power_init(void);

/* Call once per frame. `input` is non-zero when a button was pressed or the nub moved; `busy` is non-zero
 * while audio plays or the microphone records. Lowers the CPU clock after a quiet spell and restores it on
 * input or work. With keep_screen_on, also holds the backlight on; otherwise the system's own backlight
 * auto-off (XMB Power Save Settings) dims it after inactivity. The PSP never auto-sleeps while running. */
void power_tick(uint64_t now_us, int input, int busy, int cpu_saver, int keep_screen_on);

/* Restores the full clock before work that needs it (pairing crypto, saves). */
void power_full_speed(void);

/* Returns 1 once after the PSP resumed from sleep: sockets and the Wi-Fi association are gone. */
int power_take_resumed(void);

void power_get(power_status *out);

/* System WLAN power save (XMB setting, applied by the system on the next Wi-Fi connection). */
int power_wlan_saving(void);
int power_set_wlan_saving(int on);

/* Small battery badge (icon + percent) with its right edge at x. */
void power_draw_badge(int right_x, int y);

#endif
