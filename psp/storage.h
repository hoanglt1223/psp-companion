#ifndef PSPC_STORAGE_H
#define PSPC_STORAGE_H

#include <stdint.h>

typedef struct {
    uint8_t host_id[16];
    uint8_t device_id[16];
    uint8_t key[32];
    uint32_t host_ip; /* network byte order, last known address */
    uint16_t control_port;
    char host_name[33];
} pairing_record;

typedef struct {
    uint8_t keep_screen_on; /* 0: the system backlight auto-off dims the screen when idle */
    uint8_t cpu_saver;      /* lower the CPU clock after 15 s without input or audio */
} app_settings;

/* Remembers the EBOOT directory (argv[0]), where versions before 0.2 kept pairing.dat. */
void storage_init(const char *argv0);

/* Loads the save (ms0:/PSP/SAVEDATA/PSPCOMPAN0000) through the system save utility and moves an old
 * pairing.dat into it. Needs ui_init(); runs a few frames of its own. Returns 0 when a save exists. */
int storage_open(void);

/* 0 and the record when paired, -1 otherwise. */
int storage_load(pairing_record *rec);
int storage_save(const pairing_record *rec);
/* Removes the pairing and keeps the settings. */
void storage_forget(void);

const app_settings *storage_settings(void);
int storage_save_settings(const app_settings *s);

#endif
