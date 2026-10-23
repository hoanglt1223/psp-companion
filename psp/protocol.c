#include "protocol.h"

#include <string.h>

static const uint8_t MAGIC[4] = {'P', 'S', 'P', 'C'};

typedef struct {
    const uint8_t *p;
    size_t n;
    int err;
} reader;

static const uint8_t *rd_bytes(reader *r, size_t k)
{
    const uint8_t *at;
    if (r->err || r->n < k) {
        r->err = 1;
        return NULL;
    }
    at = r->p;
    r->p += k;
    r->n -= k;
    return at;
}

static uint8_t rd_u8(reader *r)
{
    const uint8_t *b = rd_bytes(r, 1);
    return b ? b[0] : 0;
}

static uint16_t rd_u16(reader *r)
{
    const uint8_t *b = rd_bytes(r, 2);
    return b ? pspc_get16(b) : 0;
}

static uint32_t rd_u32(reader *r)
{
    const uint8_t *b = rd_bytes(r, 4);
    return b ? pspc_get32(b) : 0;
}

static uint64_t rd_u64(reader *r)
{
    const uint8_t *b = rd_bytes(r, 8);
    return b ? pspc_get64(b) : 0;
}

static void rd_copy(reader *r, uint8_t *dst, size_t k)
{
    const uint8_t *b = rd_bytes(r, k);
    if (b) memcpy(dst, b, k);
}

static void rd_str(reader *r, size_t len, char *dst, size_t cap)
{
    const uint8_t *b = rd_bytes(r, len);
    size_t keep = len < cap - 1 ? len : cap - 1;
    if (!b) keep = 0;
    if (keep) memcpy(dst, b, keep);
    dst[keep] = '\0';
}

static int rd_done(const reader *r)
{
    return (r->err || r->n != 0) ? PSPC_EMALFORMED : PSPC_OK;
}

/* Returns the number of bytes of s that fit in max without splitting a UTF-8 sequence. */
static size_t utf8_prefix(const char *s, size_t max)
{
    size_t n = strlen(s);
    if (n <= max) return n;
    n = max;
    while (n > 0 && ((unsigned char)s[n] & 0xC0) == 0x80) n--;
    return n;
}

static size_t put_str8(uint8_t *out, const char *s, size_t max)
{
    size_t n = utf8_prefix(s, max);
    out[0] = (uint8_t)n;
    memcpy(out + 1, s, n);
    return 1 + n;
}

static size_t put_header(uint8_t *out, uint8_t kind)
{
    memcpy(out, MAGIC, 4);
    out[4] = PSPC_PROTO_VERSION;
    out[5] = kind;
    return 6;
}

/* Checks magic, version and kind; returns PSPC_OK, PSPC_EVERSION or PSPC_EMALFORMED. */
static int rd_header(reader *r, uint8_t kind)
{
    const uint8_t *m = rd_bytes(r, 4);
    uint8_t version, k;
    if (!m || memcmp(m, MAGIC, 4) != 0) return PSPC_EMALFORMED;
    version = rd_u8(r);
    if (r->err) return PSPC_EMALFORMED;
    if (version != PSPC_PROTO_VERSION) return PSPC_EVERSION;
    k = rd_u8(r);
    return (r->err || k != kind) ? PSPC_EMALFORMED : PSPC_OK;
}

size_t pspc_build_discover(uint8_t out[PSPC_DISCOVER_LEN], const uint8_t nonce[8])
{
    size_t n = put_header(out, 0x01);
    memcpy(out + n, nonce, 8);
    return n + 8;
}

int pspc_parse_announce(const uint8_t *buf, size_t len, pspc_announce *a)
{
    reader r = {buf, len, 0};
    int rc = rd_header(&r, 0x02);
    if (rc != PSPC_OK) return rc;
    rd_copy(&r, a->nonce, 8);
    rd_copy(&r, a->host_id, 16);
    a->control_port = rd_u16(&r);
    a->audio_port = rd_u16(&r);
    a->flags = rd_u8(&r);
    rd_str(&r, rd_u8(&r), a->host_name, sizeof a->host_name);
    return rd_done(&r);
}

size_t pspc_build_session_hello(uint8_t *out, const uint8_t device_id[16], const uint8_t nonce[16])
{
    size_t n = put_header(out, 0x01);
    memcpy(out + n, device_id, 16);
    memcpy(out + n + 16, nonce, 16);
    return n + 32;
}

/* A version-mismatch reply carries the host's version, so check it before the kind. */
int pspc_parse_session_reply(const uint8_t *buf, size_t len, uint8_t *status, uint8_t host_id[16],
                             uint8_t server_nonce[16])
{
    reader r = {buf, len, 0};
    int rc = rd_header(&r, 0x02);
    if (rc != PSPC_OK) return rc;
    *status = rd_u8(&r);
    rd_copy(&r, host_id, 16);
    rd_copy(&r, server_nonce, 16);
    return rd_done(&r);
}

size_t pspc_build_pair_request(uint8_t *out, const uint8_t device_id[16], const uint8_t pa[32], const char *name)
{
    size_t n = put_header(out, 0x03);
    memcpy(out + n, device_id, 16);
    memcpy(out + n + 16, pa, 32);
    return n + 48 + put_str8(out + n + 48, name, 32);
}

int pspc_parse_pair_challenge(const uint8_t *buf, size_t len, uint8_t *status, uint8_t host_id[16], uint8_t pb[32],
                              uint8_t cb[32])
{
    reader r = {buf, len, 0};
    int rc = rd_header(&r, 0x04);
    if (rc != PSPC_OK) return rc;
    *status = rd_u8(&r);
    rd_copy(&r, host_id, 16);
    rd_copy(&r, pb, 32);
    rd_copy(&r, cb, 32);
    return rd_done(&r);
}

size_t pspc_build_pair_confirm(uint8_t *out, const uint8_t ca[32])
{
    size_t n = put_header(out, 0x05);
    memcpy(out + n, ca, 32);
    return n + 32;
}

int pspc_parse_pair_result(const uint8_t *buf, size_t len, uint8_t *status)
{
    reader r = {buf, len, 0};
    int rc = rd_header(&r, 0x06);
    if (rc != PSPC_OK) return rc;
    *status = rd_u8(&r);
    return rd_done(&r);
}

size_t pspc_build_hello(uint8_t *out, uint32_t caps, const char *sw_version, const char *device_name)
{
    size_t n = 0;
    out[n++] = PSPC_MSG_HELLO;
    pspc_put32(out + n, caps);
    n += 4;
    n += put_str8(out + n, sw_version, 255);
    n += put_str8(out + n, device_name, 32);
    return n;
}

size_t pspc_build_ping(uint8_t *out, uint32_t id, uint64_t t_us)
{
    out[0] = PSPC_MSG_PING;
    pspc_put32(out + 1, id);
    pspc_put64(out + 5, t_us);
    return 13;
}

size_t pspc_build_bye(uint8_t *out, uint8_t reason)
{
    out[0] = PSPC_MSG_BYE;
    out[1] = reason;
    return 2;
}

size_t pspc_build_audio_register(uint8_t *out)
{
    out[0] = PSPC_MSG_AUDIO_REGISTER;
    return 1;
}

static int rd_type(reader *r, uint8_t type)
{
    uint8_t t = rd_u8(r);
    return (!r->err && t == type) ? PSPC_OK : PSPC_EMALFORMED;
}

int pspc_parse_hello_ack(const uint8_t *buf, size_t len, pspc_hello_ack *a)
{
    reader r = {buf, len, 0};
    if (rd_type(&r, PSPC_MSG_HELLO_ACK) != PSPC_OK) return PSPC_EMALFORMED;
    a->caps = rd_u32(&r);
    rd_str(&r, rd_u8(&r), a->host_name, sizeof a->host_name);
    a->session_tag = rd_u32(&r);
    a->audio_port = rd_u16(&r);
    a->keepalive_s = rd_u16(&r);
    a->budget_kbps = rd_u16(&r);
    return rd_done(&r);
}

int pspc_parse_pong(const uint8_t *buf, size_t len, uint32_t *id, uint64_t *t_us)
{
    reader r = {buf, len, 0};
    if (rd_type(&r, PSPC_MSG_PONG) != PSPC_OK) return PSPC_EMALFORMED;
    *id = rd_u32(&r);
    *t_us = rd_u64(&r);
    return rd_done(&r);
}

int pspc_parse_bye(const uint8_t *buf, size_t len, uint8_t *reason)
{
    reader r = {buf, len, 0};
    if (rd_type(&r, PSPC_MSG_BYE) != PSPC_OK) return PSPC_EMALFORMED;
    *reason = rd_u8(&r);
    return rd_done(&r);
}

int pspc_parse_error(const uint8_t *buf, size_t len, uint16_t *code, char *text, size_t text_cap)
{
    reader r = {buf, len, 0};
    if (rd_type(&r, PSPC_MSG_ERROR) != PSPC_OK) return PSPC_EMALFORMED;
    *code = rd_u16(&r);
    rd_str(&r, rd_u16(&r), text, text_cap);
    return rd_done(&r);
}

/* ---------- Widgets ---------- */

static void rd_str8(reader *r, char *dst, size_t cap)
{
    rd_str(r, rd_u8(r), dst, cap);
}

int pspc_parse_widget_layout(const uint8_t *buf, size_t len, uint8_t *format, const uint8_t **data,
                             size_t *data_len)
{
    reader r = {buf, len, 0};
    if (rd_type(&r, PSPC_MSG_WIDGET_LAYOUT) != PSPC_OK) return PSPC_EMALFORMED;
    *format = rd_u8(&r);
    if (r.err) return PSPC_EMALFORMED;
    *data = r.p;
    *data_len = r.n;
    return PSPC_OK;
}

int pspc_parse_widget_data(const uint8_t *buf, size_t len, uint16_t *widget_id, uint8_t *format,
                           const uint8_t **data, size_t *data_len)
{
    reader r = {buf, len, 0};
    if (rd_type(&r, PSPC_MSG_WIDGET_DATA) != PSPC_OK) return PSPC_EMALFORMED;
    *widget_id = rd_u16(&r);
    *format = rd_u8(&r);
    if (r.err) return PSPC_EMALFORMED;
    *data = r.p;
    *data_len = r.n;
    return PSPC_OK;
}

int pspc_parse_widget_asset(const uint8_t *buf, size_t len, uint64_t *asset_id, uint8_t *format,
                            const uint8_t **data, size_t *data_len)
{
    reader r = {buf, len, 0};
    if (rd_type(&r, PSPC_MSG_WIDGET_ASSET) != PSPC_OK) return PSPC_EMALFORMED;
    *asset_id = rd_u64(&r);
    *format = rd_u8(&r);
    if (r.err) return PSPC_EMALFORMED;
    *data = r.p;
    *data_len = r.n;
    return PSPC_OK;
}

int pspc_parse_layout(const uint8_t *data, size_t len, pspc_layout *out)
{
    reader r = {data, len, 0};
    int i;
    memset(out, 0, sizeof *out);
    out->page_count = rd_u8(&r);
    if (r.err || out->page_count < 1 || out->page_count > PSPC_MAX_PAGES) return PSPC_EMALFORMED;
    for (i = 0; i < out->page_count; i++) rd_str8(&r, out->pages[i], sizeof out->pages[i]);
    out->widget_count = rd_u8(&r);
    if (r.err || out->widget_count > PSPC_MAX_WIDGETS) return PSPC_EMALFORMED;
    for (i = 0; i < out->widget_count; i++) {
        pspc_widget_def *d = &out->widgets[i];
        d->id = rd_u16(&r);
        d->kind = rd_u8(&r);
        d->page = rd_u8(&r);
        d->x = rd_u16(&r);
        d->y = rd_u16(&r);
        d->w = rd_u16(&r);
        d->h = rd_u16(&r);
        d->flags = rd_u8(&r);
        rd_str8(&r, d->title, sizeof d->title);
        if (r.err || d->page >= out->page_count || d->w == 0 || d->h == 0 || d->x + d->w > 480 || d->y + d->h > 272)
            return PSPC_EMALFORMED;
    }
    out->macro_count = rd_u8(&r);
    if (r.err || out->macro_count > PSPC_MAX_MACROS) return PSPC_EMALFORMED;
    for (i = 0; i < out->macro_count; i++) {
        out->macros[i].slot = rd_u8(&r);
        rd_str8(&r, out->macros[i].label, sizeof out->macros[i].label);
        if (out->macros[i].slot >= PSPC_MAX_MACROS) return PSPC_EMALFORMED;
    }
    return rd_done(&r);
}

int pspc_parse_widget_value(uint8_t kind, const uint8_t *data, size_t len, pspc_widget_value *out)
{
    reader r = {data, len, 0};
    int i;
    memset(out, 0, sizeof *out);
    out->kind = kind;
    switch (kind) {
    case PSPC_W_SYSTEM:
        out->u.system.cpu_pct = rd_u8(&r);
        out->u.system.gpu_pct = rd_u8(&r);
        out->u.system.ram_used_mb = rd_u32(&r);
        out->u.system.ram_total_mb = rd_u32(&r);
        out->u.system.cpu_temp = (int16_t)rd_u16(&r);
        out->u.system.gpu_temp = (int16_t)rd_u16(&r);
        break;
    case PSPC_W_NOW_PLAYING:
        out->u.now.state = rd_u8(&r);
        out->u.now.cover = rd_u64(&r);
        rd_str8(&r, out->u.now.title, sizeof out->u.now.title);
        rd_str8(&r, out->u.now.artist, sizeof out->u.now.artist);
        break;
    case PSPC_W_NOTIFICATIONS:
        out->u.notes.count = rd_u8(&r);
        if (out->u.notes.count > 5) return PSPC_EMALFORMED;
        for (i = 0; i < out->u.notes.count; i++) {
            rd_str8(&r, out->u.notes.items[i].app, sizeof out->u.notes.items[i].app);
            rd_str8(&r, out->u.notes.items[i].title, sizeof out->u.notes.items[i].title);
            rd_str8(&r, out->u.notes.items[i].body, sizeof out->u.notes.items[i].body);
            rd_str8(&r, out->u.notes.items[i].time, sizeof out->u.notes.items[i].time);
        }
        break;
    case PSPC_W_CALENDAR:
        out->u.calendar.has_event = rd_u8(&r);
        if (out->u.calendar.has_event > 1) return PSPC_EMALFORMED;
        if (out->u.calendar.has_event) {
            rd_str8(&r, out->u.calendar.title, sizeof out->u.calendar.title);
            rd_str8(&r, out->u.calendar.when, sizeof out->u.calendar.when);
        }
        break;
    case PSPC_W_CLAUDE_CODE:
        out->u.claude.count = rd_u8(&r);
        if (out->u.claude.count > 4) return PSPC_EMALFORMED;
        for (i = 0; i < out->u.claude.count; i++) {
            rd_str8(&r, out->u.claude.items[i].project, sizeof out->u.claude.items[i].project);
            out->u.claude.items[i].state = rd_u8(&r);
            rd_str8(&r, out->u.claude.items[i].detail, sizeof out->u.claude.items[i].detail);
        }
        break;
    default:
        return PSPC_EMALFORMED;
    }
    return rd_done(&r);
}

size_t pspc_build_input_event(uint8_t *out, uint8_t kind, uint8_t page, uint16_t widget_id, uint16_t x, uint16_t y,
                              uint32_t buttons)
{
    out[0] = PSPC_MSG_INPUT_EVENT;
    out[1] = kind;
    out[2] = page;
    pspc_put16(out + 3, widget_id);
    pspc_put16(out + 5, x);
    pspc_put16(out + 7, y);
    pspc_put32(out + 9, buttons);
    return 13;
}

size_t pspc_build_macro_trigger(uint8_t *out, uint8_t slot)
{
    out[0] = PSPC_MSG_MACRO_TRIGGER;
    out[1] = slot;
    return 2;
}

/* ---------- Audio streams ---------- */

int pspc_parse_stream_start(const uint8_t *buf, size_t len, pspc_stream_start *out)
{
    reader r = {buf, len, 0};
    if (rd_type(&r, PSPC_MSG_AUDIO_STREAM_START) != PSPC_OK) return PSPC_EMALFORMED;
    out->stream_id = rd_u8(&r);
    out->kind = rd_u8(&r);
    out->codec = rd_u8(&r);
    out->sample_rate = rd_u32(&r);
    out->channels = rd_u8(&r);
    out->bitrate_kbps = rd_u16(&r);
    return rd_done(&r);
}

int pspc_parse_stream_stop(const uint8_t *buf, size_t len, uint8_t *stream_id, uint8_t *reason)
{
    reader r = {buf, len, 0};
    if (rd_type(&r, PSPC_MSG_AUDIO_STREAM_STOP) != PSPC_OK) return PSPC_EMALFORMED;
    *stream_id = rd_u8(&r);
    *reason = rd_u8(&r);
    return rd_done(&r);
}

int pspc_parse_audio_packet(const uint8_t *buf, size_t len, uint8_t *stream_id, uint32_t *seq, uint32_t *pts_ms,
                            const uint8_t **data, size_t *data_len)
{
    reader r = {buf, len, 0};
    if (rd_type(&r, PSPC_MSG_AUDIO_PACKET) != PSPC_OK) return PSPC_EMALFORMED;
    *stream_id = rd_u8(&r);
    *seq = rd_u32(&r);
    *pts_ms = rd_u32(&r);
    if (r.err) return PSPC_EMALFORMED;
    *data = r.p;
    *data_len = r.n;
    return PSPC_OK;
}

size_t pspc_build_stream_request(uint8_t *out, uint8_t kind, uint8_t enable)
{
    out[0] = PSPC_MSG_AUDIO_STREAM_REQUEST;
    out[1] = kind;
    out[2] = enable ? 1 : 0;
    return 3;
}

size_t pspc_build_stream_stop(uint8_t *out, uint8_t stream_id, uint8_t reason)
{
    out[0] = PSPC_MSG_AUDIO_STREAM_STOP;
    out[1] = stream_id;
    out[2] = reason;
    return 3;
}

/* ---------- Voice assistant ---------- */

size_t pspc_build_voice_start(uint8_t *out, uint32_t utterance_id, uint32_t sample_rate)
{
    out[0] = PSPC_MSG_VOICE_START;
    pspc_put32(out + 1, utterance_id);
    out[5] = PSPC_VOICE_CODEC_ADPCM;
    pspc_put32(out + 6, sample_rate);
    out[10] = 1; /* mono */
    return 11;
}

size_t pspc_build_voice_chunk_header(uint8_t *out, uint32_t utterance_id, uint32_t seq)
{
    out[0] = PSPC_MSG_VOICE_CHUNK;
    pspc_put32(out + 1, utterance_id);
    pspc_put32(out + 5, seq);
    return 9;
}

size_t pspc_build_voice_end(uint8_t *out, uint32_t utterance_id, uint8_t reason)
{
    out[0] = PSPC_MSG_VOICE_END;
    pspc_put32(out + 1, utterance_id);
    out[5] = reason;
    return 6;
}

int pspc_parse_voice_text(const uint8_t *buf, size_t len, uint8_t *type, uint32_t *utterance_id, uint8_t *is_final,
                          char *text, size_t text_cap)
{
    reader r = {buf, len, 0};
    *type = rd_u8(&r);
    if (r.err || (*type != PSPC_MSG_VOICE_TRANSCRIPT && *type != PSPC_MSG_VOICE_REPLY_TEXT)) return PSPC_EMALFORMED;
    *utterance_id = rd_u32(&r);
    *is_final = rd_u8(&r);
    if (*is_final > 1) return PSPC_EMALFORMED;
    rd_str(&r, rd_u16(&r), text, text_cap);
    return rd_done(&r);
}
