/* PSP Companion client: Wi-Fi, discovery, pairing, and the connection status screen. */
#include <pspctrl.h>
#include <pspkernel.h>
#include <psputility.h>
#include <stdio.h>
#include <string.h>

#include "net.h"
#include "rng.h"
#include "storage.h"
#include "ui.h"
#include "wifi.h"

PSP_MODULE_INFO("PSP Companion", PSP_MODULE_USER, 0, 1);
PSP_MAIN_THREAD_ATTR(PSP_THREAD_ATTR_USER | PSP_THREAD_ATTR_VFPU);
PSP_HEAP_SIZE_KB(-1024);

#define APP_VERSION "0.1.0"
#define WIFI_RECONNECT_TIMEOUT_US (30 * 1000 * 1000ULL)

enum app_state {
    ST_WIFI_DIALOG,
    ST_WIFI_WAIT,
    ST_WIFI_RECONNECT,
    ST_DISCOVER,
    ST_FIND_HOST,
    ST_CONNECTING,
    ST_CONNECTED,
    ST_BACKOFF
};

static volatile int g_running = 1;
static enum app_state g_state = ST_WIFI_DIALOG;
static pairing_record g_rec;
static int g_paired;
static char g_device_name[33] = "PSP";
static char g_msg[128];
static int g_selected;
static int g_failures;
static uint64_t g_backoff_until;
static uint64_t g_reconnect_started;
static int g_forget_armed;

static int exit_callback(int arg1, int arg2, void *common)
{
    (void)arg1;
    (void)arg2;
    (void)common;
    g_running = 0;
    return 0;
}

static int callback_thread(SceSize args, void *argp)
{
    int cbid = sceKernelCreateCallback("Exit Callback", exit_callback, NULL);
    (void)args;
    (void)argp;
    sceKernelRegisterExitCallback(cbid);
    sceKernelSleepThreadCB();
    return 0;
}

static void setup_callbacks(void)
{
    int th = sceKernelCreateThread("update_thread", callback_thread, 0x11, 0xFA0, PSP_THREAD_ATTR_USER, NULL);
    if (th >= 0) sceKernelStartThread(th, 0, NULL);
}

static void set_msg(const char *m)
{
    snprintf(g_msg, sizeof g_msg, "%s", m);
}

static void header(void)
{
    int strength = wifi_strength();
    ui_text(1, 1, UI_CYAN, "PSP Companion %s", APP_VERSION);
    if (strength >= 0)
        ui_text(46, 1, UI_GREY, "Wi-Fi %3d%%", strength);
    else
        ui_text(46, 1, UI_RED, "Wi-Fi  off");
    ui_text(1, 2, UI_GREY, "------------------------------------------------------------");
}

static void footer(const char *keys)
{
    if (g_msg[0]) ui_text(1, 30, UI_YELLOW, "%s", g_msg);
    ui_text(1, 32, UI_GREY, "%s", keys);
}

static void simple_screen(uint32_t color, const char *line1, const char *line2, const char *keys)
{
    ui_begin();
    header();
    ui_text(1, 5, color, "%s", line1);
    if (line2) ui_text(1, 7, UI_WHITE, "%s", line2);
    footer(keys);
    ui_end();
}

static void enter_backoff(void)
{
    static const unsigned steps[] = {1, 2, 4, 8, 16, 30};
    unsigned i = g_failures < 6 ? (unsigned)g_failures : 5;
    g_backoff_until = net_now_us() + (uint64_t)steps[i] * 1000000ULL;
    if (g_failures < 100) g_failures++;
    g_state = ST_BACKOFF;
}

static void start_discovery(enum app_state next)
{
    if (net_discovery_start() != 0) set_msg("Cannot open a UDP socket for discovery.");
    g_selected = 0;
    g_state = next;
}

static void forget_pairing(const char *why)
{
    storage_forget();
    memset(&g_rec, 0, sizeof g_rec);
    g_paired = 0;
    set_msg(why);
    start_discovery(ST_DISCOVER);
}

static void do_pair(const char *host_name, uint32_t ip, uint16_t port)
{
    char pin[16], digits[8], err[96];
    int i, n = 0;
    if (ui_osk("Enter the 6-digit PIN shown on the PC", "", 7, pin, sizeof pin) != 0) return;
    for (i = 0; pin[i] && n < 7; i++)
        if (pin[i] != ' ') digits[n++] = pin[i];
    digits[n] = '\0';

    simple_screen(UI_YELLOW, "Pairing... this takes a few seconds.", host_name, "");
    if (net_pair(ip, port, host_name, digits, g_device_name, &g_rec, err, sizeof err) == NET_OK) {
        g_paired = 1;
        g_failures = 0;
        snprintf(g_msg, sizeof g_msg, "Paired with %s.", host_name);
        net_discovery_stop();
        g_state = ST_CONNECTING;
    } else {
        set_msg(err);
    }
}

static void manual_ip(void)
{
    char text[24];
    uint32_t ip;
    if (ui_osk("PC IP address (e.g. 192.168.1.10)", "192.168.", 15, text, sizeof text) != 0) return;
    if (net_parse_ip(text, &ip) != 0) {
        set_msg("That is not a valid IPv4 address.");
        return;
    }
    if (g_paired) {
        g_rec.host_ip = ip;
        g_rec.control_port = PSPC_PORT_CONTROL;
        if (storage_save(&g_rec) != 0) set_msg("Could not save the new address.");
        net_discovery_stop();
        g_state = ST_CONNECTING;
    } else {
        do_pair(text, ip, PSPC_PORT_CONTROL);
    }
}

static void screen_discover(uint32_t pressed)
{
    int count, i;
    net_discovery_poll();
    count = net_discovery_count();

    if (count > 0) {
        if (pressed & PSP_CTRL_UP) g_selected = (g_selected + count - 1) % count;
        if (pressed & PSP_CTRL_DOWN) g_selected = (g_selected + 1) % count;
        if (g_selected >= count) g_selected = 0;
    }
    if (pressed & PSP_CTRL_CIRCLE) start_discovery(ST_DISCOVER);
    if (pressed & PSP_CTRL_TRIANGLE) {
        manual_ip();
        return;
    }
    if ((pressed & PSP_CTRL_CROSS) && count > 0) {
        const net_host *h = net_discovery_host(g_selected);
        do_pair(h->a.host_name, h->ip, h->a.control_port);
        return;
    }

    ui_begin();
    header();
    ui_text(1, 4, UI_WHITE, "Pair with a PC. On the PC, choose 'Pair new PSP' in the tray.");
    ui_text(1, 6, UI_GREY, net_discovery_active() ? "Searching the local network..." : "Search finished.");
    for (i = 0; i < count; i++) {
        const net_host *h = net_discovery_host(i);
        char ip[16];
        net_ip_to_str(h->ip, ip);
        ui_text(2, 8 + i * 2, i == g_selected ? UI_GREEN : UI_WHITE, "%c %-24s %-15s %s", i == g_selected ? '>' : ' ',
                h->a.host_name, ip, (h->a.flags & PSPC_FLAG_PAIRING_OPEN) ? "PIN shown" : "");
    }
    if (count == 0 && !net_discovery_active())
        ui_text(2, 8, UI_YELLOW, "No PC found. Is companion-host running and the firewall open?");
    footer("X Pair   O Search again   /\\ Enter IP   START Wi-Fi   HOME Quit");
    ui_end();
}

static void screen_find_host(void)
{
    int i;
    net_discovery_poll();
    for (i = 0; i < net_discovery_count(); i++) {
        const net_host *h = net_discovery_host(i);
        if (memcmp(h->a.host_id, g_rec.host_id, 16) == 0) {
            g_rec.host_ip = h->ip;
            g_rec.control_port = h->a.control_port;
            snprintf(g_rec.host_name, sizeof g_rec.host_name, "%s", h->a.host_name);
            if (storage_save(&g_rec) != 0) set_msg("Could not save the new PC address.");
            net_discovery_stop();
            g_state = ST_CONNECTING;
            return;
        }
    }
    if (!net_discovery_active()) {
        net_discovery_stop();
        enter_backoff();
        return;
    }
    simple_screen(UI_YELLOW, "Looking for the paired PC on the network...", g_rec.host_name, "HOME Quit");
}

static void screen_connecting(void)
{
    char err[96], line[64];
    int rc;
    snprintf(line, sizeof line, "Connecting to %s...", g_rec.host_name);
    simple_screen(UI_YELLOW, line, NULL, "");
    rc = net_session_open(&g_rec, g_device_name, err, sizeof err);
    if (rc == NET_OK) {
        g_failures = 0;
        g_forget_armed = 0;
        g_msg[0] = '\0';
        g_state = ST_CONNECTED;
    } else if (rc == NET_ERR_UNPAIRED) {
        forget_pairing(err);
    } else {
        set_msg(err);
        enter_backoff();
    }
}

static void fmt_bytes(char *out, size_t cap, uint64_t b)
{
    if (b < 10 * 1024)
        snprintf(out, cap, "%u B", (unsigned)b);
    else if (b < 10 * 1024 * 1024)
        snprintf(out, cap, "%u KB", (unsigned)(b / 1024));
    else
        snprintf(out, cap, "%u MB", (unsigned)(b / (1024 * 1024)));
}

static void screen_connected(uint32_t pressed)
{
    const net_stats *s;
    char ip[16], tin[16], tout[16];
    uint64_t secs;
    int rc = net_session_poll();

    if (rc != NET_OK) {
        set_msg(net_get_stats()->status);
        if (rc == NET_ERR_UNPAIRED) {
            forget_pairing(net_get_stats()->status);
        } else if (!wifi_has_ip()) {
            g_reconnect_started = 0;
            g_state = ST_WIFI_RECONNECT;
        } else {
            enter_backoff();
        }
        return;
    }
    if (pressed & PSP_CTRL_SQUARE) net_session_ping_now();
    if (pressed & PSP_CTRL_START) {
        net_session_close(PSPC_BYE_NORMAL);
        g_state = ST_WIFI_DIALOG;
        return;
    }
    if (pressed & PSP_CTRL_SELECT) {
        if (g_forget_armed) {
            net_session_close(PSPC_BYE_NORMAL);
            forget_pairing("Pairing forgotten. Pair again to reconnect.");
            return;
        }
        g_forget_armed = 1;
        set_msg("Press SELECT again to forget this pairing.");
    } else if (pressed) {
        g_forget_armed = 0;
        if (strcmp(g_msg, "Press SELECT again to forget this pairing.") == 0) g_msg[0] = '\0';
    }

    s = net_get_stats();
    secs = (net_now_us() - s->connected_since_us) / 1000000ULL;
    wifi_ip(ip);
    fmt_bytes(tin, sizeof tin, s->total_in);
    fmt_bytes(tout, sizeof tout, s->total_out);

    ui_begin();
    header();
    ui_text(1, 4, UI_GREEN, "Connected");
    ui_text(1, 6, UI_WHITE, "PC         %s (%s)", s->host_name, s->host_ip);
    ui_text(1, 7, UI_WHITE, "This PSP   %s  %s", ip, wifi_profile_name());
    ui_text(1, 8, UI_WHITE, "Session    %02u:%02u:%02u", (unsigned)(secs / 3600), (unsigned)(secs / 60 % 60),
            (unsigned)(secs % 60));
    if (s->rtt_ms >= 0)
        ui_text(1, 10, UI_WHITE, "RTT        %d ms   (pings %u, pongs %u)", s->rtt_ms, (unsigned)s->pings,
                (unsigned)s->pongs);
    else
        ui_text(1, 10, UI_GREY, "RTT        waiting for first keepalive (press [])");
    ui_text(1, 12, UI_WHITE, "Throughput in %6.2f kbps   out %6.2f kbps", (double)s->kbps_in, (double)s->kbps_out);
    ui_text(1, 13, (s->kbps_in + s->kbps_out) > (float)s->budget_kbps ? UI_RED : UI_WHITE,
            "           total %6.2f of %u kbps budget", (double)(s->kbps_in + s->kbps_out), (unsigned)s->budget_kbps);
    ui_text(1, 14, UI_GREY, "Totals     in %s   out %s", tin, tout);
    ui_text(1, 16, UI_WHITE, "Audio UDP  %s   packets %u   dropped %u", s->udp_registered ? "ready" : "registering",
            (unsigned)s->udp_packets, (unsigned)s->udp_dropped);
    ui_text(1, 17, UI_GREY, "Keepalive  every %u s; data is pushed by the PC only on change", (unsigned)s->keepalive_s);
    footer("[] Measure RTT   SELECT Forget pairing   START Wi-Fi   HOME Quit");
    ui_end();
}

static void screen_backoff(uint32_t pressed)
{
    uint64_t now = net_now_us();
    char line[64];
    if ((pressed & PSP_CTRL_CROSS) || now >= g_backoff_until) {
        if (!wifi_has_ip()) {
            g_reconnect_started = 0;
            g_state = ST_WIFI_RECONNECT;
        } else if (g_failures >= 2 && g_failures % 2 == 0) {
            /* The PC may have a new DHCP address; find it by its host ID. */
            start_discovery(ST_FIND_HOST);
        } else {
            g_state = ST_CONNECTING;
        }
        return;
    }
    if (pressed & PSP_CTRL_START) {
        g_state = ST_WIFI_DIALOG;
        return;
    }
    snprintf(line, sizeof line, "Retrying in %u s (attempt %d)", (unsigned)((g_backoff_until - now) / 1000000ULL) + 1,
             g_failures);
    simple_screen(UI_YELLOW, "Not connected to the PC.", line, "X Retry now   START Wi-Fi   HOME Quit");
}

static void screen_wifi_reconnect(uint32_t pressed)
{
    uint64_t now = net_now_us();
    if (pressed & PSP_CTRL_START) {
        g_state = ST_WIFI_DIALOG;
        return;
    }
    if (!wifi_switch_on()) {
        simple_screen(UI_RED, "The WLAN switch is off.", "Turn it on to reconnect.", "START Wi-Fi   HOME Quit");
        g_reconnect_started = 0;
        return;
    }
    if (g_reconnect_started == 0) {
        if (wifi_reconnect_start() < 0) {
            g_state = ST_WIFI_DIALOG;
            return;
        }
        g_reconnect_started = now;
    }
    if (wifi_has_ip()) {
        g_failures = 0;
        if (g_paired)
            g_state = ST_CONNECTING;
        else
            start_discovery(ST_DISCOVER);
        return;
    }
    if (now - g_reconnect_started > WIFI_RECONNECT_TIMEOUT_US) {
        g_reconnect_started = 0;
        enter_backoff();
        return;
    }
    simple_screen(UI_YELLOW, "Wi-Fi dropped. Reconnecting...", wifi_profile_name(), "START Choose network   HOME Quit");
}

int main(int argc, char *argv[])
{
    int rc, mic;
    setup_callbacks();
    storage_init(argc > 0 ? argv[0] : NULL);
    ui_init();

    simple_screen(UI_WHITE, "Starting...", NULL, "");
    mic = rng_init();
    if (sceUtilityGetSystemParamString(PSP_SYSTEMPARAM_ID_STRING_NICKNAME, g_device_name, sizeof g_device_name) < 0)
        snprintf(g_device_name, sizeof g_device_name, "PSP");
    if (mic == 0) set_msg("Note: microphone unavailable; using timer entropy only.");

    rc = wifi_init();
    if (rc < 0) {
        char line[64];
        snprintf(line, sizeof line, "Network init failed (0x%08X).", (unsigned)rc);
        while (g_running) simple_screen(UI_RED, line, "Restart the PSP and try again.", "HOME Quit");
        sceKernelExitGame();
        return 0;
    }
    g_paired = storage_load(&g_rec) == 0;

    while (g_running) {
        uint32_t pressed = ui_poll_pressed();
        net_stats_tick();
        if ((pressed & PSP_CTRL_START) && (g_state == ST_DISCOVER || g_state == ST_WIFI_WAIT)) {
            net_discovery_stop();
            g_state = ST_WIFI_DIALOG;
        }
        switch (g_state) {
        case ST_WIFI_DIALOG:
            if (!wifi_switch_on()) {
                set_msg("Turn on the WLAN switch, then press X.");
                g_state = ST_WIFI_WAIT;
            } else if (wifi_dialog() == 0) {
                g_msg[0] = '\0';
                g_failures = 0;
                if (g_paired)
                    g_state = ST_CONNECTING;
                else
                    start_discovery(ST_DISCOVER);
            } else {
                g_state = ST_WIFI_WAIT;
            }
            break;
        case ST_WIFI_WAIT:
            if (pressed & PSP_CTRL_CROSS) g_state = ST_WIFI_DIALOG;
            simple_screen(UI_YELLOW, "Not connected to Wi-Fi.",
                          "Use a 2.4 GHz network with WPA2-AES (see README).", "X Choose network   HOME Quit");
            break;
        case ST_WIFI_RECONNECT:
            screen_wifi_reconnect(pressed);
            break;
        case ST_DISCOVER:
            screen_discover(pressed);
            break;
        case ST_FIND_HOST:
            screen_find_host();
            break;
        case ST_CONNECTING:
            screen_connecting();
            break;
        case ST_CONNECTED:
            screen_connected(pressed);
            break;
        case ST_BACKOFF:
            screen_backoff(pressed);
            break;
        }
    }

    net_session_close(PSPC_BYE_NORMAL);
    net_discovery_stop();
    wifi_term();
    sceKernelExitGame();
    return 0;
}
