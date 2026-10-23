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

/* ---------- Widgets (phase 03) ---------- */

#define PSPC_FORMAT_BINARY 1
#define PSPC_INPUT_TAP 0

#define PSPC_MAX_PAGES 3
#define PSPC_MAX_WIDGETS 16
#define PSPC_MAX_MACROS 12

enum pspc_widget_kind {
    PSPC_W_CLOCK = 1,
    PSPC_W_SYSTEM = 2,
    PSPC_W_NOW_PLAYING = 3,
    PSPC_W_NOTIFICATIONS = 4,
    PSPC_W_CALENDAR = 5,
    PSPC_W_CLAUDE_CODE = 6
};

#define PSPC_WIDGET_TAPPABLE 0x01
#define PSPC_TEMP_UNKNOWN (-32768)
#define PSPC_PCT_UNKNOWN 255

typedef struct {
    uint16_t id;
    uint8_t kind;
    uint8_t page;
    uint16_t x, y, w, h;
    uint8_t flags;
    char title[33];
} pspc_widget_def;

typedef struct {
    uint8_t page_count;
    char pages[PSPC_MAX_PAGES][25];
    uint8_t widget_count;
    pspc_widget_def widgets[PSPC_MAX_WIDGETS];
    uint8_t macro_count;
    struct {
        uint8_t slot;
        char label[25];
    } macros[PSPC_MAX_MACROS];
} pspc_layout;

typedef struct {
    uint8_t cpu_pct, gpu_pct;
    uint32_t ram_used_mb, ram_total_mb;
    int16_t cpu_temp, gpu_temp; /* tenths of a degree, PSPC_TEMP_UNKNOWN */
} pspc_w_system;

typedef struct {
    uint8_t state; /* 0 nothing, 1 playing, 2 paused, 3 stopped */
    uint64_t cover;
    char title[97];
    char artist[65];
} pspc_w_now_playing;

typedef struct {
    uint8_t count;
    struct {
        char app[33], title[65], body[161], time[17];
    } items[5];
} pspc_w_notifications;

typedef struct {
    uint8_t has_event;
    char title[97];
    char when[33];
} pspc_w_calendar;

typedef struct {
    uint8_t count;
    struct {
        char project[41];
        uint8_t state; /* 0 idle, 1 working, 2 waiting, 3 done */
        char detail[65];
    } items[4];
} pspc_w_claude;

typedef struct {
    uint8_t kind;
    union {
        pspc_w_system system;
        pspc_w_now_playing now;
        pspc_w_notifications notes;
        pspc_w_calendar calendar;
        pspc_w_claude claude;
    } u;
} pspc_widget_value;

/* Envelopes: *data points into buf (the `rest` field). */
int pspc_parse_widget_layout(const uint8_t *buf, size_t len, uint8_t *format, const uint8_t **data,
                             size_t *data_len);
int pspc_parse_widget_data(const uint8_t *buf, size_t len, uint16_t *widget_id, uint8_t *format,
                           const uint8_t **data, size_t *data_len);
int pspc_parse_widget_asset(const uint8_t *buf, size_t len, uint64_t *asset_id, uint8_t *format,
                            const uint8_t **data, size_t *data_len);
/* Format 1 bodies. */
int pspc_parse_layout(const uint8_t *data, size_t len, pspc_layout *out);
int pspc_parse_widget_value(uint8_t kind, const uint8_t *data, size_t len, pspc_widget_value *out);

size_t pspc_build_input_event(uint8_t *out, uint8_t kind, uint8_t page, uint16_t widget_id, uint16_t x, uint16_t y,
                              uint32_t buttons);
size_t pspc_build_macro_trigger(uint8_t *out, uint8_t slot);

/* ---------- Audio streams (phases 02 and 04) ---------- */

#define PSPC_AUDIO_KIND_VOICE 0
#define PSPC_AUDIO_KIND_SPEAKER 1
#define PSPC_AUDIO_CODEC_MP3 1
#define PSPC_STOP_ENDED 0
#define PSPC_STOP_USER 1
#define PSPC_STOP_PREEMPTED 2

typedef struct {
    uint8_t stream_id, kind, codec, channels;
    uint32_t sample_rate;
    uint16_t bitrate_kbps;
} pspc_stream_start;

int pspc_parse_stream_start(const uint8_t *buf, size_t len, pspc_stream_start *out);
int pspc_parse_stream_stop(const uint8_t *buf, size_t len, uint8_t *stream_id, uint8_t *reason);
/* *data points into buf. */
int pspc_parse_audio_packet(const uint8_t *buf, size_t len, uint8_t *stream_id, uint32_t *seq, uint32_t *pts_ms,
                            const uint8_t **data, size_t *data_len);
size_t pspc_build_stream_request(uint8_t *out, uint8_t kind, uint8_t enable);
size_t pspc_build_stream_stop(uint8_t *out, uint8_t stream_id, uint8_t reason);

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
