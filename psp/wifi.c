#include "wifi.h"

#include <pspkernel.h>
#include <pspnet.h>
#include <pspnet_apctl.h>
#include <pspnet_inet.h>
#include <psputility.h>
#include <psputility_netmodules.h>
#include <psputility_netparam.h>
#include <pspwlan.h>
#include <string.h>

#include "ui.h"

/* sceUtilityLoadNetModule returns this when the module is already resident. */
#define ERR_MODULE_ALREADY_LOADED ((int)0x80110802)
#define IP_TIMEOUT_US (30 * 1000 * 1000)

static int g_profile = -1;
static char g_profile_name[64];

int wifi_init(void)
{
    int rc = sceUtilityLoadNetModule(PSP_NET_MODULE_COMMON);
    if (rc < 0 && rc != ERR_MODULE_ALREADY_LOADED) return rc;
    rc = sceUtilityLoadNetModule(PSP_NET_MODULE_INET);
    if (rc < 0 && rc != ERR_MODULE_ALREADY_LOADED) return rc;
    if ((rc = sceNetInit(128 * 1024, 42, 4 * 1024, 42, 4 * 1024)) < 0) return rc;
    if ((rc = sceNetInetInit()) < 0) return rc;
    if ((rc = sceNetApctlInit(0x8000, 48)) < 0) return rc;
    return 0;
}

void wifi_term(void)
{
    sceNetApctlDisconnect();
    sceNetApctlTerm();
    sceNetInetTerm();
    sceNetTerm();
}

int wifi_state(void)
{
    int state = PSP_NET_APCTL_STATE_DISCONNECTED;
    if (sceNetApctlGetState(&state) < 0) return PSP_NET_APCTL_STATE_DISCONNECTED;
    return state;
}

int wifi_has_ip(void)
{
    return wifi_state() == PSP_NET_APCTL_STATE_GOT_IP;
}

int wifi_switch_on(void)
{
    return sceWlanGetSwitchState() == 1;
}

int wifi_strength(void)
{
    SceNetApctlInfo info;
    if (!wifi_has_ip() || sceNetApctlGetInfo(PSP_NET_APCTL_INFO_STRENGTH, &info) < 0) return -1;
    return info.strength;
}

void wifi_ip(char out[16])
{
    SceNetApctlInfo info;
    out[0] = '\0';
    if (sceNetApctlGetInfo(PSP_NET_APCTL_INFO_IP, &info) == 0) {
        memcpy(out, info.ip, 15);
        out[15] = '\0';
    }
}

const char *wifi_profile_name(void)
{
    return g_profile_name;
}

/* Maps the connected profile's name back to its index so reconnects can skip the dialog. */
static void remember_profile(void)
{
    SceNetApctlInfo info;
    netData name;
    int i;
    g_profile = -1;
    g_profile_name[0] = '\0';
    if (sceNetApctlGetInfo(PSP_NET_APCTL_INFO_PROFILE_NAME, &info) < 0) return;
    memcpy(g_profile_name, info.name, sizeof g_profile_name - 1);
    g_profile_name[sizeof g_profile_name - 1] = '\0';
    for (i = 1; i <= 24; i++) {
        if (sceUtilityCheckNetParam(i) != 0) continue;
        if (sceUtilityGetNetParam(i, PSP_NETPARAM_NAME, &name) == 0 && strcmp(name.asString, g_profile_name) == 0) {
            g_profile = i;
            return;
        }
    }
}

static int wait_for_ip(void)
{
    SceInt64 start = sceKernelGetSystemTimeWide();
    while (sceKernelGetSystemTimeWide() - start < IP_TIMEOUT_US) {
        int state = wifi_state();
        if (state == PSP_NET_APCTL_STATE_GOT_IP) return 0;
        ui_begin();
        ui_text(2, 2, UI_WHITE, "PSP Companion");
        ui_text(2, 5, UI_YELLOW, "Connecting to Wi-Fi... (state %d)", state);
        ui_end();
    }
    return -1;
}

int wifi_dialog(void)
{
    pspUtilityNetconfData data;
    struct pspUtilityNetconfAdhoc adhoc;
    int done = 0;

    memset(&data, 0, sizeof data);
    memset(&adhoc, 0, sizeof adhoc);
    data.base.size = sizeof data;
    sceUtilityGetSystemParamInt(PSP_SYSTEMPARAM_ID_INT_LANGUAGE, &data.base.language);
    sceUtilityGetSystemParamInt(PSP_SYSTEMPARAM_ID_INT_BUTTON_SWAP, &data.base.buttonSwap);
    data.base.graphicsThread = 17;
    data.base.accessThread = 19;
    data.base.fontThread = 18;
    data.base.soundThread = 16;
    data.action = PSP_NETCONF_ACTION_CONNECTAP;
    data.adhocparam = &adhoc;

    if (sceUtilityNetconfInitStart(&data) < 0) return -1;
    while (!done) {
        ui_dialog_frame_begin();
        switch (sceUtilityNetconfGetStatus()) {
        case PSP_UTILITY_DIALOG_VISIBLE:
            sceUtilityNetconfUpdate(1);
            break;
        case PSP_UTILITY_DIALOG_QUIT:
            sceUtilityNetconfShutdownStart();
            break;
        case PSP_UTILITY_DIALOG_NONE:
            done = 1;
            break;
        default:
            break;
        }
        ui_dialog_frame_end();
    }
    if (wifi_state() == PSP_NET_APCTL_STATE_DISCONNECTED) return -1;
    if (wait_for_ip() != 0) return -1;
    remember_profile();
    return 0;
}

int wifi_reconnect_start(void)
{
    if (g_profile < 0) return -1;
    if (wifi_state() != PSP_NET_APCTL_STATE_DISCONNECTED) sceNetApctlDisconnect();
    return sceNetApctlConnect(g_profile);
}
