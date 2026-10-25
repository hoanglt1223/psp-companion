#include "menu.h"

#include <pspctrl.h>
#include <stdio.h>

#include "power.h"
#include "storage.h"
#include "ui.h"

enum page { PAGE_HOME, PAGE_SETTINGS };

enum home_item { HOME_SCREEN, HOME_ASSISTANT, HOME_MACROS, HOME_DECK, HOME_SPEAKER, HOME_SETTINGS, HOME_COUNT };

enum settings_item { SET_STATUS, SET_SCREEN_ON, SET_CPU_SAVER, SET_WLAN_SAVE, SET_WIFI, SET_FORGET, SET_COUNT };

static int g_open;
static enum page g_page;
static int g_sel[2];
static int g_forget_armed;
static int g_dirty;
static app_settings g_edit;
static const char *g_note;

static const char *home_label(int i)
{
    switch (i) {
    case HOME_SCREEN: return "Screen (widgets)";
    case HOME_ASSISTANT: return "Assistant (hold R to talk)";
    case HOME_MACROS: return "Macros (12-slot pad)";
    case HOME_DECK: return "Stream Deck (Bitfocus Companion)";
    case HOME_SPEAKER: return "PC speaker";
    case HOME_SETTINGS: return "Settings";
    default: return "";
    }
}

void menu_open(void)
{
    g_open = 1;
    g_page = PAGE_HOME;
    g_forget_armed = 0;
    g_note = NULL;
}

static void leave_settings(void)
{
    if (g_dirty && storage_save_settings(&g_edit) != 0) g_note = "Could not save the settings.";
    g_dirty = 0;
    g_forget_armed = 0;
}

void menu_close(void)
{
    if (g_page == PAGE_SETTINGS) leave_settings();
    g_open = 0;
}

int menu_is_open(void)
{
    return g_open;
}

static menu_action finish(menu_action a)
{
    menu_close();
    return a;
}

static menu_action settings_select(void)
{
    int w;
    switch (g_sel[PAGE_SETTINGS]) {
    case SET_STATUS: return finish(MENU_STATUS);
    case SET_SCREEN_ON:
        g_edit.keep_screen_on = !g_edit.keep_screen_on;
        g_dirty = 1;
        break;
    case SET_CPU_SAVER:
        g_edit.cpu_saver = !g_edit.cpu_saver;
        g_dirty = 1;
        break;
    case SET_WLAN_SAVE:
        w = power_wlan_saving();
        g_note = power_set_wlan_saving(w != 1) < 0 ? "The system refused to change WLAN power save."
                                                   : "Applies the next time the PSP joins Wi-Fi.";
        break;
    case SET_WIFI: return finish(MENU_WIFI);
    case SET_FORGET:
        if (g_forget_armed) return finish(MENU_FORGET);
        g_forget_armed = 1;
        break;
    default: break;
    }
    return MENU_NONE;
}

menu_action menu_input(uint32_t pressed)
{
    int count = g_page == PAGE_HOME ? HOME_COUNT : SET_COUNT;
    int *sel = &g_sel[g_page];
    if (!g_open) return MENU_NONE;
    if (pressed & PSP_CTRL_START) return finish(MENU_CLOSE);
    if (pressed & PSP_CTRL_UP) *sel = (*sel + count - 1) % count;
    if (pressed & PSP_CTRL_DOWN) *sel = (*sel + 1) % count;
    if (pressed & (PSP_CTRL_UP | PSP_CTRL_DOWN)) {
        g_forget_armed = 0;
        g_note = NULL;
    }
    if (pressed & PSP_CTRL_CIRCLE) {
        if (g_page == PAGE_HOME) return finish(MENU_CLOSE);
        leave_settings();
        g_page = PAGE_HOME;
        return MENU_NONE;
    }
    if (!(pressed & PSP_CTRL_CROSS)) return MENU_NONE;
    if (g_page == PAGE_SETTINGS) return settings_select();
    switch (*sel) {
    case HOME_SCREEN: return finish(MENU_SCREEN);
    case HOME_ASSISTANT: return finish(MENU_ASSISTANT);
    case HOME_MACROS: return finish(MENU_MACROS);
    case HOME_DECK: return finish(MENU_DECK);
    case HOME_SPEAKER: return finish(MENU_SPEAKER);
    case HOME_SETTINGS:
        g_edit = *storage_settings();
        g_dirty = 0;
        g_note = NULL;
        g_page = PAGE_SETTINGS;
        return MENU_NONE;
    default: return MENU_NONE;
    }
}

static void item(int i, int row, const char *label, const char *value)
{
    int on = i == g_sel[g_page];
    ui_text(3, row, on ? UI_GREEN : UI_WHITE, "%c %-30s %s", on ? '>' : ' ', label, value ? value : "");
}

static void draw_settings(void)
{
    power_status p;
    int wlan = power_wlan_saving();
    ui_text(1, 4, UI_WHITE, "Settings");
    item(SET_STATUS, 6, "Connection status", NULL);
    item(SET_SCREEN_ON, 8, "Keep the screen on", g_edit.keep_screen_on ? "on" : "off (system auto-off)");
    item(SET_CPU_SAVER, 10, "Lower CPU clock when idle", g_edit.cpu_saver ? "on" : "off");
    item(SET_WLAN_SAVE, 12, "WLAN power save (system)", wlan < 0 ? "unknown" : wlan ? "on" : "off");
    item(SET_WIFI, 14, "Choose Wi-Fi network", NULL);
    item(SET_FORGET, 16, "Forget this PC (pair again)", g_forget_armed ? "press X again" : NULL);

    power_get(&p);
    if (p.percent >= 0) {
        if (p.minutes > 0)
            ui_text(3, 20, UI_GREY, "Battery %d%%, about %d h %02d min left", p.percent, p.minutes / 60, p.minutes % 60);
        else
            ui_text(3, 20, UI_GREY, "Battery %d%%%s", p.percent, p.charging ? ", charging" : p.on_ac ? ", on AC" : "");
    } else {
        ui_text(3, 20, UI_GREY, "No battery (AC power)");
    }
    ui_text(3, 21, UI_GREY, "CPU %d MHz", p.cpu_mhz);
    ui_text(3, 23, UI_GREY, "Screen auto-off follows XMB Settings > Power Save Settings.");
    if (g_note) ui_text(3, 26, UI_YELLOW, "%s", g_note);
}

void menu_draw(void)
{
    int i;
    if (g_page == PAGE_SETTINGS) {
        draw_settings();
        return;
    }
    ui_text(1, 4, UI_WHITE, "Menu");
    for (i = 0; i < HOME_COUNT; i++) item(i, 6 + i * 2, home_label(i), NULL);
    if (g_note) ui_text(3, 26, UI_YELLOW, "%s", g_note);
}

const char *menu_keys(void)
{
    return g_page == PAGE_HOME ? "X Select   O Close   START Close   HOME Quit"
                               : "X Change   O Back   START Close   HOME Quit";
}
