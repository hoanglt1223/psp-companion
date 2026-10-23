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

/* Uses the directory of the running EBOOT (argv[0]). */
void storage_init(const char *argv0);
/* 0 on success, -1 if missing or corrupt. */
int storage_load(pairing_record *rec);
int storage_save(const pairing_record *rec);
void storage_forget(void);
const char *storage_path(void);

#endif
