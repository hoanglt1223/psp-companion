/* The pairing key and the settings live in one save made by the system save utility, so the XMB lists
 * it as "PSP Companion" save data (with PARAM.SFO and icon) and the user can copy or delete it there.
 * The utility handles the PSP's save encryption; a SHA-256 over the body catches anything else. */
#include "storage.h"

#include <pspkernel.h>
#include <psputility.h>
#include <stdio.h>
#include <string.h>

#include "crypto.h"
#include "icon0_png.h"
#include "protocol.h"
#include "ui.h"

#define SAVE_GAME_NAME "PSPCOMPAN"
#define SAVE_NAME "0000"
#define SAVE_FILE "DATA.BIN"

#define MAGIC "PSPCSAVE"
#define FILE_VERSION 1
#define FLAG_PAIRED 0x01
#define REC_LEN (16 + 16 + 32 + 4 + 2 + 33)
#define SETTINGS_LEN 8
#define BODY_LEN (8 + 1 + 1 + REC_LEN + SETTINGS_LEN)
#define FILE_LEN (BODY_LEN + 32)

/* Format used next to the EBOOT before the save utility. */
#define LEGACY_MAGIC "PSPCPAIR"
#define LEGACY_BODY_LEN (8 + 1 + REC_LEN)
#define LEGACY_LEN (LEGACY_BODY_LEN + 32)


static char g_legacy_path[256] = "ms0:/PSP/GAME/PSPCompanion/pairing.dat";
static pairing_record g_rec;
static int g_paired;
static app_settings g_settings = {0, 1};
static uint8_t __attribute__((aligned(64))) g_buf[FILE_LEN + 64];

void storage_init(const char *argv0)
{
    const char *slash = argv0 ? strrchr(argv0, '/') : NULL;
    size_t dir_len;
    if (slash) {
        dir_len = (size_t)(slash - argv0);
        if (dir_len + sizeof "/pairing.dat" <= sizeof g_legacy_path) {
            memcpy(g_legacy_path, argv0, dir_len);
            strcpy(g_legacy_path + dir_len, "/pairing.dat");
        }
    }
}

static void put_record(uint8_t *p, const pairing_record *r)
{
    memcpy(p, r->host_id, 16); p += 16;
    memcpy(p, r->device_id, 16); p += 16;
    memcpy(p, r->key, 32); p += 32;
    memcpy(p, &r->host_ip, 4); p += 4;
    pspc_put16(p, r->control_port); p += 2;
    memset(p, 0, 33);
    memcpy(p, r->host_name, strnlen(r->host_name, 32));
}

static void get_record(const uint8_t *p, pairing_record *r)
{
    memcpy(r->host_id, p, 16); p += 16;
    memcpy(r->device_id, p, 16); p += 16;
    memcpy(r->key, p, 32); p += 32;
    memcpy(&r->host_ip, p, 4); p += 4;
    r->control_port = pspc_get16(p); p += 2;
    memcpy(r->host_name, p, 33);
    r->host_name[32] = '\0';
}

static void encode(uint8_t out[FILE_LEN])
{
    uint8_t *p = out;
    memset(out, 0, FILE_LEN);
    memcpy(p, MAGIC, 8); p += 8;
    *p++ = FILE_VERSION;
    *p++ = g_paired ? FLAG_PAIRED : 0;
    if (g_paired) put_record(p, &g_rec);
    p += REC_LEN;
    p[0] = g_settings.keep_screen_on;
    p[1] = g_settings.cpu_saver;
    pspc_sha256(out, BODY_LEN, out + BODY_LEN);
}

static int decode(const uint8_t *buf, size_t n)
{
    uint8_t sum[32];
    const uint8_t *p = buf + 10;
    if (n != FILE_LEN || memcmp(buf, MAGIC, 8) != 0 || buf[8] != FILE_VERSION) return -1;
    pspc_sha256(buf, BODY_LEN, sum);
    if (!pspc_ct_equal(sum, buf + BODY_LEN, 32)) return -1;
    g_paired = (buf[9] & FLAG_PAIRED) != 0;
    if (g_paired) get_record(p, &g_rec);
    p += REC_LEN;
    g_settings.keep_screen_on = p[0] != 0;
    g_settings.cpu_saver = p[1] != 0;
    return 0;
}

/* Runs the save utility without UI (auto load or auto save). Returns its result code (0 on success). */
static int run_utility(int mode, size_t *data_size)
{
    SceUtilitySavedataParam p;
    int done = 0, rc;

    memset(&p, 0, sizeof p);
    p.base.size = sizeof p;
    sceUtilityGetSystemParamInt(PSP_SYSTEMPARAM_ID_INT_LANGUAGE, &p.base.language);
    sceUtilityGetSystemParamInt(PSP_SYSTEMPARAM_ID_INT_BUTTON_SWAP, &p.base.buttonSwap);
    p.base.graphicsThread = 17;
    p.base.accessThread = 19;
    p.base.fontThread = 18;
    p.base.soundThread = 16;
    p.mode = mode;
    p.overwrite = 1;
    strcpy(p.gameName, SAVE_GAME_NAME);
    strcpy(p.saveName, SAVE_NAME);
    strcpy(p.fileName, SAVE_FILE);
    /* Version 1 is the PSP's keyless save encryption; 0 would require a per-game key on SDK 4+ firmware. */
    p.secureVersion = 1;
    p.dataBuf = g_buf;
    p.dataBufSize = sizeof g_buf;
    p.dataSize = mode == PSP_UTILITY_SAVEDATA_AUTOSAVE ? FILE_LEN : 0;
    snprintf(p.sfoParam.title, sizeof p.sfoParam.title, "PSP Companion");
    snprintf(p.sfoParam.savedataTitle, sizeof p.sfoParam.savedataTitle, "Pairing and settings");
    snprintf(p.sfoParam.detail, sizeof p.sfoParam.detail,
             "The pairing with your PC and the app settings. Without it the PSP has to pair again.");
    p.sfoParam.parentalLevel = 1;
    p.icon0FileData.buf = (void *)icon0_png;
    p.icon0FileData.bufSize = (SceSize)size_icon0_png;
    p.icon0FileData.size = (SceSize)size_icon0_png;

    rc = sceUtilitySavedataInitStart(&p);
    if (rc < 0) return rc;
    while (!done) {
        ui_begin();
        ui_text(1, 16, UI_GREY, mode == PSP_UTILITY_SAVEDATA_AUTOSAVE ? "Saving..." : "Loading...");
        switch (sceUtilitySavedataGetStatus()) {
        case PSP_UTILITY_DIALOG_VISIBLE:
            sceUtilitySavedataUpdate(1);
            break;
        case PSP_UTILITY_DIALOG_QUIT:
            sceUtilitySavedataShutdownStart();
            break;
        case PSP_UTILITY_DIALOG_NONE:
            done = 1;
            break;
        default:
            break;
        }
        ui_end();
    }
    if (data_size) *data_size = p.dataSize;
    return p.base.result;
}

static int write_save(void)
{
    int rc;
    encode(g_buf);
    rc = run_utility(PSP_UTILITY_SAVEDATA_AUTOSAVE, NULL);
    pspc_wipe(g_buf, sizeof g_buf);
    return rc == 0 ? 0 : -1;
}

static int load_legacy(void)
{
    uint8_t buf[LEGACY_LEN + 1], sum[32];
    size_t n;
    FILE *f = fopen(g_legacy_path, "rb");
    if (!f) return -1;
    n = fread(buf, 1, sizeof buf, f);
    fclose(f);
    if (n != LEGACY_LEN || memcmp(buf, LEGACY_MAGIC, 8) != 0 || buf[8] != 1) return -1;
    pspc_sha256(buf, LEGACY_BODY_LEN, sum);
    if (!pspc_ct_equal(sum, buf + LEGACY_BODY_LEN, 32)) return -1;
    get_record(buf + 9, &g_rec);
    pspc_wipe(buf, sizeof buf);
    g_paired = 1;
    return 0;
}

int storage_open(void)
{
    size_t n = 0;
    int loaded = run_utility(PSP_UTILITY_SAVEDATA_AUTOLOAD, &n) == 0 && decode(g_buf, n) == 0;
    pspc_wipe(g_buf, sizeof g_buf);
    if (!loaded) {
        g_paired = 0;
        memset(&g_rec, 0, sizeof g_rec);
    }
    if (!g_paired && load_legacy() == 0) {
        /* The old file is removed only once the save holds the key, so a failed write loses nothing. */
        if (write_save() == 0) {
            remove(g_legacy_path);
            loaded = 1;
        }
    }
    return loaded ? 0 : -1;
}

int storage_load(pairing_record *r)
{
    if (!g_paired) return -1;
    *r = g_rec;
    return 0;
}

int storage_save(const pairing_record *r)
{
    g_rec = *r;
    g_paired = 1;
    return write_save();
}

void storage_forget(void)
{
    pspc_wipe(&g_rec, sizeof g_rec);
    g_paired = 0;
    write_save();
    remove(g_legacy_path);
}

const app_settings *storage_settings(void)
{
    return &g_settings;
}

int storage_save_settings(const app_settings *s)
{
    g_settings = *s;
    return write_save();
}
