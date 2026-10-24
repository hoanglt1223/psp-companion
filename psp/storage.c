/* The pairing record lives next to the EBOOT rather than in PSP/SAVEDATA: a raw
 * file without PARAM.SFO there shows up as corrupted data in the XMB. */
#include "storage.h"

#include <stdio.h>
#include <string.h>

#include "crypto.h"
#include "protocol.h"

#define MAGIC "PSPCPAIR"
#define FILE_VERSION 1
#define BODY_LEN (8 + 1 + 16 + 16 + 32 + 4 + 2 + 33)
#define FILE_LEN (BODY_LEN + 32)

static char g_path[256] = "ms0:/PSP/GAME/PSPCompanion/pairing.dat";
static char g_tmp[260];

void storage_init(const char *argv0)
{
    const char *slash = argv0 ? strrchr(argv0, '/') : NULL;
    size_t dir_len;
    if (slash) {
        dir_len = (size_t)(slash - argv0);
        if (dir_len + sizeof "/pairing.dat" <= sizeof g_path) {
            memcpy(g_path, argv0, dir_len);
            strcpy(g_path + dir_len, "/pairing.dat");
        }
    }
    snprintf(g_tmp, sizeof g_tmp, "%s.tmp", g_path);
}

const char *storage_path(void)
{
    return g_path;
}

static void encode(const pairing_record *r, uint8_t out[FILE_LEN])
{
    uint8_t *p = out;
    memcpy(p, MAGIC, 8); p += 8;
    *p++ = FILE_VERSION;
    memcpy(p, r->host_id, 16); p += 16;
    memcpy(p, r->device_id, 16); p += 16;
    memcpy(p, r->key, 32); p += 32;
    memcpy(p, &r->host_ip, 4); p += 4;
    pspc_put16(p, r->control_port); p += 2;
    memset(p, 0, 33);
    memcpy(p, r->host_name, strnlen(r->host_name, 32));
    pspc_sha256(out, BODY_LEN, out + BODY_LEN);
}

int storage_load(pairing_record *r)
{
    uint8_t buf[FILE_LEN + 1], sum[32];
    const uint8_t *p = buf;
    size_t n;
    FILE *f = fopen(g_path, "rb");
    if (!f) return -1;
    n = fread(buf, 1, sizeof buf, f);
    fclose(f);
    if (n != FILE_LEN || memcmp(buf, MAGIC, 8) != 0 || buf[8] != FILE_VERSION) return -1;
    pspc_sha256(buf, BODY_LEN, sum);
    if (!pspc_ct_equal(sum, buf + BODY_LEN, 32)) return -1;
    p += 9;
    memcpy(r->host_id, p, 16); p += 16;
    memcpy(r->device_id, p, 16); p += 16;
    memcpy(r->key, p, 32); p += 32;
    memcpy(&r->host_ip, p, 4); p += 4;
    r->control_port = pspc_get16(p); p += 2;
    memcpy(r->host_name, p, 33);
    r->host_name[32] = '\0';
    pspc_wipe(buf, sizeof buf);
    return 0;
}

/* Write-then-rename so a battery pull mid-write never destroys the old pairing. */
int storage_save(const pairing_record *r)
{
    uint8_t buf[FILE_LEN];
    FILE *f;
    int ok;
    encode(r, buf);
    f = fopen(g_tmp, "wb");
    if (!f) return -1;
    ok = fwrite(buf, 1, FILE_LEN, f) == FILE_LEN;
    ok = (fclose(f) == 0) && ok;
    pspc_wipe(buf, sizeof buf);
    if (!ok) {
        remove(g_tmp);
        return -1;
    }
    remove(g_path);
    return rename(g_tmp, g_path) == 0 ? 0 : -1;
}

void storage_forget(void)
{
    remove(g_path);
}
