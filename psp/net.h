#ifndef PSPC_NET_H
#define PSPC_NET_H

#include <stddef.h>
#include <stdint.h>

#include "protocol.h"
#include "storage.h"

#define NET_OK 0
#define NET_ERR (-1)          /* transient: retry with back-off */
#define NET_ERR_UNPAIRED (-2) /* the host does not know this PSP: pair again */
#define NET_ERR_VERSION (-3)  /* incompatible host version */

#define NET_MAX_HOSTS 8

typedef struct {
    uint32_t ip; /* network byte order */
    pspc_announce a;
} net_host;

typedef struct {
    int connected;
    int udp_registered;
    char host_name[33];
    char host_ip[16];
    uint16_t keepalive_s;
    uint16_t budget_kbps;
    int rtt_ms; /* -1 until the first Pong */
    float kbps_in;
    float kbps_out;
    uint64_t total_in;
    uint64_t total_out;
    uint32_t pings;
    uint32_t pongs;
    uint32_t udp_packets;
    uint32_t udp_dropped;
    uint64_t connected_since_us;
    char status[96]; /* last host error or disconnect reason */
} net_stats;

uint64_t net_now_us(void);
void net_ip_to_str(uint32_t ip, char out[16]);
/* Parses dotted IPv4. Returns 0 on success. */
int net_parse_ip(const char *s, uint32_t *ip);

int net_discovery_start(void);
/* Sends at most one request per second, five in total, and collects replies. */
void net_discovery_poll(void);
int net_discovery_active(void);
void net_discovery_stop(void);
int net_discovery_count(void);
const net_host *net_discovery_host(int i);

/* Blocking pairing exchange (a few seconds, mostly the SPAKE2 maths).
 * On success the record is filled and saved. err receives a user-facing message. */
int net_pair(uint32_t ip, uint16_t port, const char *host_name, const char *pin, const char *device_name,
             pairing_record *rec, char *err, size_t err_cap);

/* Blocking connect and handshake (up to about 15 seconds). */
int net_session_open(const pairing_record *rec, const char *device_name, char *err, size_t err_cap);
/* Non-blocking; call once per frame. Returns NET_OK, or an error once the session has dropped. */
int net_session_poll(void);
/* User-requested RTT measurement; ignored if called more than once per second. */
void net_session_ping_now(void);
void net_session_close(uint8_t bye_reason);

/* Queues one plaintext message on the control channel (sent by the next net_session_poll).
 * Returns 0, or -1 when not connected or the send queue is full. */
int net_send(const uint8_t *msg, size_t len);

/* Receives every control message the network layer does not handle itself (widgets, voice, deck). */
typedef void (*net_message_fn)(const uint8_t *msg, size_t len);
void net_set_message_handler(net_message_fn fn);

const net_stats *net_get_stats(void);
/* Updates the per-second throughput figures; call once per frame. */
void net_stats_tick(void);

#endif
