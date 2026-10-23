#ifndef PSPC_WIFI_H
#define PSPC_WIFI_H

/* Loads the network modules and initialises the stack. Returns < 0 on failure. */
int wifi_init(void);
void wifi_term(void);

/* Shows the system access-point selection dialog and waits for an IP.
 * Returns 0 when connected, < 0 when cancelled or failed. */
int wifi_dialog(void);

/* Reconnects to the profile chosen in the last dialog without showing UI.
 * Returns < 0 if there is no remembered profile. Poll wifi_has_ip() afterwards. */
int wifi_reconnect_start(void);

int wifi_has_ip(void);
int wifi_state(void);
int wifi_switch_on(void);
/* Signal strength in percent, or -1 when unknown. */
int wifi_strength(void);
void wifi_ip(char out[16]);
const char *wifi_profile_name(void);

#endif
