/* Wire constants and encoders/decoders for protocol/PROTOCOL.md, version 1.
 * Pure C99 (no PSP headers) so the host test harness can check it against the Rust host. */
#ifndef PSPC_PROTOCOL_H
#define PSPC_PROTOCOL_H

#include <stddef.h>
#include <stdint.h>

#define PSPC_PROTO_VERSION 1

#define PSPC_PORT_DISCOVERY 47860
#define PSPC_PORT_CONTROL 47861
#define PSPC_PORT_AUDIO 47862

#define PSPC_MAX_MESSAGE 16384
#define PSPC_MAX_FRAME_BODY (PSPC_MAX_MESSAGE + 16)
#define PSPC_UDP_HEADER 13
#define PSPC_MAX_DATAGRAM 1200
#define PSPC_DISCOVER_LEN 14
#define PSPC_FLAG_PAIRING_OPEN 0x01

/* Wire-size estimates shared with the host rate limiter. */
#define PSPC_TCP_OVERHEAD 40
#define PSPC_UDP_OVERHEAD 28

enum pspc_status {
    PSPC_ST_OK = 0,
    PSPC_ST_UNKNOWN = 1,
    PSPC_ST_VERSION = 2,
    PSPC_ST_TOO_MANY_ATTEMPTS = 3,
    PSPC_ST_WRONG_PIN = 4,
    PSPC_ST_BUSY = 5
};

enum pspc_msg_type {
    PSPC_MSG_HELLO = 0x01,
    PSPC_MSG_HELLO_ACK = 0x02,
    PSPC_MSG_PING = 0x03,
    PSPC_MSG_PONG = 0x04,
    PSPC_MSG_BYE = 0x05,
    PSPC_MSG_ERROR = 0x06,
    PSPC_MSG_WIDGET_LAYOUT = 0x10,
    PSPC_MSG_WIDGET_DATA = 0x11,
    PSPC_MSG_WIDGET_ASSET = 0x12,
    PSPC_MSG_INPUT_EVENT = 0x20,
    PSPC_MSG_MACRO_TRIGGER = 0x21,
    PSPC_MSG_VOICE_START = 0x30,
    PSPC_MSG_VOICE_CHUNK = 0x31,
    PSPC_MSG_VOICE_END = 0x32,
    PSPC_MSG_VOICE_TRANSCRIPT = 0x33,
    PSPC_MSG_VOICE_REPLY_TEXT = 0x34,
    PSPC_MSG_AUDIO_STREAM_START = 0x40,
    PSPC_MSG_AUDIO_STREAM_STOP = 0x41,
    PSPC_MSG_AUDIO_STREAM_REQUEST = 0x42,
    PSPC_MSG_AUDIO_PACKET = 0x43,
    PSPC_MSG_AUDIO_REGISTER = 0x44,
    PSPC_MSG_AUDIO_REGISTER_ACK = 0x45,
    PSPC_MSG_DECK_LAYOUT = 0x50,
    PSPC_MSG_DECK_KEY_IMAGE = 0x51,
    PSPC_MSG_DECK_KEY_PRESS = 0x52
};

enum pspc_bye_reason { PSPC_BYE_NORMAL = 0, PSPC_BYE_SHUTDOWN = 1, PSPC_BYE_REPLACED = 2, PSPC_BYE_UNPAIRED = 3 };

/* Parse results. */
#define PSPC_OK 0
#define PSPC_EMALFORMED (-1)
#define PSPC_EVERSION (-2)

typedef struct {
    uint8_t nonce[8];
    uint8_t host_id[16];
    uint16_t control_port;
    uint16_t audio_port;
    uint8_t flags;
    char host_name[33];
} pspc_announce;

typedef struct {
    uint32_t caps;
    char host_name[33];
    uint32_t session_tag;
    uint16_t audio_port;
    uint16_t keepalive_s;
    uint16_t budget_kbps;
} pspc_hello_ack;

/* Discovery. */
size_t pspc_build_discover(uint8_t out[PSPC_DISCOVER_LEN], const uint8_t nonce[8]);
int pspc_parse_announce(const uint8_t *buf, size_t len, pspc_announce *out);

/* Cleartext handshake. Builders return the body length; out must hold 320 bytes. */
#define PSPC_HANDSHAKE_MAX 320
size_t pspc_build_session_hello(uint8_t *out, const uint8_t device_id[16], const uint8_t nonce[16]);
int pspc_parse_session_reply(const uint8_t *buf, size_t len, uint8_t *status, uint8_t host_id[16],
                             uint8_t server_nonce[16]);
size_t pspc_build_pair_request(uint8_t *out, const uint8_t device_id[16], const uint8_t pa[32], const char *name);
int pspc_parse_pair_challenge(const uint8_t *buf, size_t len, uint8_t *status, uint8_t host_id[16], uint8_t pb[32],
                              uint8_t cb[32]);
size_t pspc_build_pair_confirm(uint8_t *out, const uint8_t ca[32]);
int pspc_parse_pair_result(const uint8_t *buf, size_t len, uint8_t *status);

/* Encrypted-channel messages (plaintext). Builders return the length; out must hold 600 bytes. */
#define PSPC_SMALL_MSG_MAX 600
size_t pspc_build_hello(uint8_t *out, uint32_t caps, const char *sw_version, const char *device_name);
size_t pspc_build_ping(uint8_t *out, uint32_t id, uint64_t t_us);
size_t pspc_build_bye(uint8_t *out, uint8_t reason);
size_t pspc_build_audio_register(uint8_t *out);
int pspc_parse_hello_ack(const uint8_t *buf, size_t len, pspc_hello_ack *out);
int pspc_parse_pong(const uint8_t *buf, size_t len, uint32_t *id, uint64_t *t_us);
int pspc_parse_bye(const uint8_t *buf, size_t len, uint8_t *reason);
/* text is truncated to text_cap - 1 bytes and NUL-terminated. */
int pspc_parse_error(const uint8_t *buf, size_t len, uint16_t *code, char *text, size_t text_cap);

/* Little-endian helpers used by the network code. */
static inline void pspc_put16(uint8_t *p, uint16_t v)
{
    p[0] = (uint8_t)v;
    p[1] = (uint8_t)(v >> 8);
}

static inline uint16_t pspc_get16(const uint8_t *p)
{
    return (uint16_t)(p[0] | (p[1] << 8));
}

static inline void pspc_put32(uint8_t *p, uint32_t v)
{
    pspc_put16(p, (uint16_t)v);
    pspc_put16(p + 2, (uint16_t)(v >> 16));
}

static inline uint32_t pspc_get32(const uint8_t *p)
{
    return (uint32_t)pspc_get16(p) | ((uint32_t)pspc_get16(p + 2) << 16);
}

static inline void pspc_put64(uint8_t *p, uint64_t v)
{
    pspc_put32(p, (uint32_t)v);
    pspc_put32(p + 4, (uint32_t)(v >> 32));
}

static inline uint64_t pspc_get64(const uint8_t *p)
{
    return (uint64_t)pspc_get32(p) | ((uint64_t)pspc_get32(p + 4) << 32);
}

#endif
