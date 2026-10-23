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
