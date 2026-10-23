/* Field and group arithmetic for edwards25519 follows TweetNaCl (public domain,
 * Bernstein et al.). Poly1305 uses the 26-bit limb layout of poly1305-donna
 * (public domain). Everything else is written from the RFCs. */
#include "crypto.h"

#include <string.h>

/* ---------- helpers ---------- */

static uint32_t ld32le(const uint8_t *p)
{
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}

static void st32le(uint8_t *p, uint32_t v)
{
    p[0] = (uint8_t)v;
    p[1] = (uint8_t)(v >> 8);
    p[2] = (uint8_t)(v >> 16);
    p[3] = (uint8_t)(v >> 24);
}

static void st64le(uint8_t *p, uint64_t v)
{
    st32le(p, (uint32_t)v);
    st32le(p + 4, (uint32_t)(v >> 32));
}

static uint32_t ld32be(const uint8_t *p)
{
    return ((uint32_t)p[0] << 24) | ((uint32_t)p[1] << 16) | ((uint32_t)p[2] << 8) | (uint32_t)p[3];
}

static uint64_t ld64be(const uint8_t *p)
{
    return ((uint64_t)ld32be(p) << 32) | ld32be(p + 4);
}

static void st32be(uint8_t *p, uint32_t v)
{
    p[0] = (uint8_t)(v >> 24);
    p[1] = (uint8_t)(v >> 16);
    p[2] = (uint8_t)(v >> 8);
    p[3] = (uint8_t)v;
}

static void st64be(uint8_t *p, uint64_t v)
{
    st32be(p, (uint32_t)(v >> 32));
    st32be(p + 4, (uint32_t)v);
}

void pspc_wipe(void *p, size_t n)
{
    volatile uint8_t *v = (volatile uint8_t *)p;
    while (n--) *v++ = 0;
}

int pspc_ct_equal(const uint8_t *a, const uint8_t *b, size_t n)
{
    uint8_t d = 0;
    size_t i;
    for (i = 0; i < n; i++) d |= (uint8_t)(a[i] ^ b[i]);
    return d == 0;
}

void pspc_nonce(uint64_t counter, uint8_t out[12])
{
    memset(out, 0, 4);
    st64le(out + 4, counter);
}

/* ---------- SHA-256 ---------- */

static const uint32_t K256[64] = {
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2};

#define ROR32(x, n) (((x) >> (n)) | ((x) << (32 - (n))))

static void sha256_block(uint32_t h[8], const uint8_t *p)
{
    uint32_t w[64], a, b, c, d, e, f, g, hh, t1, t2;
    int i;
    for (i = 0; i < 16; i++) w[i] = ld32be(p + 4 * i);
    for (i = 16; i < 64; i++) {
        uint32_t s0 = ROR32(w[i - 15], 7) ^ ROR32(w[i - 15], 18) ^ (w[i - 15] >> 3);
        uint32_t s1 = ROR32(w[i - 2], 17) ^ ROR32(w[i - 2], 19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16] + s0 + w[i - 7] + s1;
    }
    a = h[0]; b = h[1]; c = h[2]; d = h[3]; e = h[4]; f = h[5]; g = h[6]; hh = h[7];
    for (i = 0; i < 64; i++) {
        t1 = hh + (ROR32(e, 6) ^ ROR32(e, 11) ^ ROR32(e, 25)) + ((e & f) ^ (~e & g)) + K256[i] + w[i];
        t2 = (ROR32(a, 2) ^ ROR32(a, 13) ^ ROR32(a, 22)) + ((a & b) ^ (a & c) ^ (b & c));
        hh = g; g = f; f = e; e = d + t1; d = c; c = b; b = a; a = t1 + t2;
    }
    h[0] += a; h[1] += b; h[2] += c; h[3] += d; h[4] += e; h[5] += f; h[6] += g; h[7] += hh;
}

void pspc_sha256_init(pspc_sha256_ctx *c)
{
    static const uint32_t iv[8] = {0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
                                   0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19};
    memcpy(c->h, iv, sizeof iv);
    c->len = 0;
    c->fill = 0;
}

void pspc_sha256_update(pspc_sha256_ctx *c, const void *data, size_t n)
{
    const uint8_t *p = (const uint8_t *)data;
    c->len += n;
    while (n > 0) {
        size_t take = 64 - c->fill;
        if (take > n) take = n;
        memcpy(c->buf + c->fill, p, take);
        c->fill += take;
        p += take;
        n -= take;
        if (c->fill == 64) {
            sha256_block(c->h, c->buf);
            c->fill = 0;
        }
    }
}

void pspc_sha256_final(pspc_sha256_ctx *c, uint8_t out[32])
{
    uint64_t bits = c->len * 8;
    uint8_t pad = 0x80, zero = 0, lenbuf[8];
    int i;
    pspc_sha256_update(c, &pad, 1);
    while (c->fill != 56) pspc_sha256_update(c, &zero, 1);
    st64be(lenbuf, bits);
    pspc_sha256_update(c, lenbuf, 8);
    for (i = 0; i < 8; i++) st32be(out + 4 * i, c->h[i]);
    pspc_wipe(c, sizeof *c);
}

void pspc_sha256(const void *data, size_t n, uint8_t out[32])
{
    pspc_sha256_ctx c;
    pspc_sha256_init(&c);
    pspc_sha256_update(&c, data, n);
    pspc_sha256_final(&c, out);
}

/* ---------- SHA-512 ---------- */

static const uint64_t K512[80] = {
    0x428a2f98d728ae22ULL, 0x7137449123ef65cdULL, 0xb5c0fbcfec4d3b2fULL, 0xe9b5dba58189dbbcULL,
    0x3956c25bf348b538ULL, 0x59f111f1b605d019ULL, 0x923f82a4af194f9bULL, 0xab1c5ed5da6d8118ULL,
    0xd807aa98a3030242ULL, 0x12835b0145706fbeULL, 0x243185be4ee4b28cULL, 0x550c7dc3d5ffb4e2ULL,
    0x72be5d74f27b896fULL, 0x80deb1fe3b1696b1ULL, 0x9bdc06a725c71235ULL, 0xc19bf174cf692694ULL,
    0xe49b69c19ef14ad2ULL, 0xefbe4786384f25e3ULL, 0x0fc19dc68b8cd5b5ULL, 0x240ca1cc77ac9c65ULL,
    0x2de92c6f592b0275ULL, 0x4a7484aa6ea6e483ULL, 0x5cb0a9dcbd41fbd4ULL, 0x76f988da831153b5ULL,
    0x983e5152ee66dfabULL, 0xa831c66d2db43210ULL, 0xb00327c898fb213fULL, 0xbf597fc7beef0ee4ULL,
    0xc6e00bf33da88fc2ULL, 0xd5a79147930aa725ULL, 0x06ca6351e003826fULL, 0x142929670a0e6e70ULL,
    0x27b70a8546d22ffcULL, 0x2e1b21385c26c926ULL, 0x4d2c6dfc5ac42aedULL, 0x53380d139d95b3dfULL,
    0x650a73548baf63deULL, 0x766a0abb3c77b2a8ULL, 0x81c2c92e47edaee6ULL, 0x92722c851482353bULL,
    0xa2bfe8a14cf10364ULL, 0xa81a664bbc423001ULL, 0xc24b8b70d0f89791ULL, 0xc76c51a30654be30ULL,
    0xd192e819d6ef5218ULL, 0xd69906245565a910ULL, 0xf40e35855771202aULL, 0x106aa07032bbd1b8ULL,
    0x19a4c116b8d2d0c8ULL, 0x1e376c085141ab53ULL, 0x2748774cdf8eeb99ULL, 0x34b0bcb5e19b48a8ULL,
    0x391c0cb3c5c95a63ULL, 0x4ed8aa4ae3418acbULL, 0x5b9cca4f7763e373ULL, 0x682e6ff3d6b2b8a3ULL,
    0x748f82ee5defb2fcULL, 0x78a5636f43172f60ULL, 0x84c87814a1f0ab72ULL, 0x8cc702081a6439ecULL,
    0x90befffa23631e28ULL, 0xa4506cebde82bde9ULL, 0xbef9a3f7b2c67915ULL, 0xc67178f2e372532bULL,
    0xca273eceea26619cULL, 0xd186b8c721c0c207ULL, 0xeada7dd6cde0eb1eULL, 0xf57d4f7fee6ed178ULL,
    0x06f067aa72176fbaULL, 0x0a637dc5a2c898a6ULL, 0x113f9804bef90daeULL, 0x1b710b35131c471bULL,
    0x28db77f523047d84ULL, 0x32caab7b40c72493ULL, 0x3c9ebe0a15c9bebcULL, 0x431d67c49c100d4cULL,
    0x4cc5d4becb3e42b6ULL, 0x597f299cfc657e2aULL, 0x5fcb6fab3ad6faecULL, 0x6c44198c4a475817ULL};

#define ROR64(x, n) (((x) >> (n)) | ((x) << (64 - (n))))

static void sha512_block(uint64_t h[8], const uint8_t *p)
{
    uint64_t w[80], a, b, c, d, e, f, g, hh, t1, t2;
    int i;
    for (i = 0; i < 16; i++) w[i] = ld64be(p + 8 * i);
    for (i = 16; i < 80; i++) {
        uint64_t s0 = ROR64(w[i - 15], 1) ^ ROR64(w[i - 15], 8) ^ (w[i - 15] >> 7);
        uint64_t s1 = ROR64(w[i - 2], 19) ^ ROR64(w[i - 2], 61) ^ (w[i - 2] >> 6);
        w[i] = w[i - 16] + s0 + w[i - 7] + s1;
    }
    a = h[0]; b = h[1]; c = h[2]; d = h[3]; e = h[4]; f = h[5]; g = h[6]; hh = h[7];
    for (i = 0; i < 80; i++) {
        t1 = hh + (ROR64(e, 14) ^ ROR64(e, 18) ^ ROR64(e, 41)) + ((e & f) ^ (~e & g)) + K512[i] + w[i];
        t2 = (ROR64(a, 28) ^ ROR64(a, 34) ^ ROR64(a, 39)) + ((a & b) ^ (a & c) ^ (b & c));
        hh = g; g = f; f = e; e = d + t1; d = c; c = b; b = a; a = t1 + t2;
    }
    h[0] += a; h[1] += b; h[2] += c; h[3] += d; h[4] += e; h[5] += f; h[6] += g; h[7] += hh;
}

void pspc_sha512_init(pspc_sha512_ctx *c)
{
    static const uint64_t iv[8] = {0x6a09e667f3bcc908ULL, 0xbb67ae8584caa73bULL, 0x3c6ef372fe94f82bULL,
                                   0xa54ff53a5f1d36f1ULL, 0x510e527fade682d1ULL, 0x9b05688c2b3e6c1fULL,
                                   0x1f83d9abfb41bd6bULL, 0x5be0cd19137e2179ULL};
    memcpy(c->h, iv, sizeof iv);
    c->len = 0;
    c->fill = 0;
}

void pspc_sha512_update(pspc_sha512_ctx *c, const void *data, size_t n)
{
    const uint8_t *p = (const uint8_t *)data;
    c->len += n;
    while (n > 0) {
        size_t take = 128 - c->fill;
        if (take > n) take = n;
        memcpy(c->buf + c->fill, p, take);
        c->fill += take;
        p += take;
        n -= take;
        if (c->fill == 128) {
            sha512_block(c->h, c->buf);
            c->fill = 0;
        }
    }
}

void pspc_sha512_final(pspc_sha512_ctx *c, uint8_t out[64])
{
    uint64_t bits = c->len * 8;
    uint8_t pad = 0x80, zero = 0, lenbuf[16];
    int i;
    pspc_sha512_update(c, &pad, 1);
    while (c->fill != 112) pspc_sha512_update(c, &zero, 1);
    memset(lenbuf, 0, 8);
    st64be(lenbuf + 8, bits);
    pspc_sha512_update(c, lenbuf, 16);
    for (i = 0; i < 8; i++) st64be(out + 8 * i, c->h[i]);
    pspc_wipe(c, sizeof *c);
}

void pspc_sha512(const void *data, size_t n, uint8_t out[64])
{
    pspc_sha512_ctx c;
    pspc_sha512_init(&c);
    pspc_sha512_update(&c, data, n);
    pspc_sha512_final(&c, out);
}

/* ---------- HMAC / HKDF ---------- */

typedef struct {
    pspc_sha256_ctx inner, outer;
} hmac_ctx;

static void hmac_init(hmac_ctx *h, const uint8_t *key, size_t key_len)
{
    uint8_t k[64], pad[64];
    int i;
    memset(k, 0, sizeof k);
    if (key_len > 64) {
        pspc_sha256(key, key_len, k);
    } else if (key_len > 0) {
        memcpy(k, key, key_len);
    }
    for (i = 0; i < 64; i++) pad[i] = (uint8_t)(k[i] ^ 0x36);
    pspc_sha256_init(&h->inner);
    pspc_sha256_update(&h->inner, pad, 64);
    for (i = 0; i < 64; i++) pad[i] = (uint8_t)(k[i] ^ 0x5c);
    pspc_sha256_init(&h->outer);
    pspc_sha256_update(&h->outer, pad, 64);
    pspc_wipe(k, sizeof k);
    pspc_wipe(pad, sizeof pad);
}

static void hmac_final(hmac_ctx *h, uint8_t out[32])
{
    uint8_t ih[32];
    pspc_sha256_final(&h->inner, ih);
    pspc_sha256_update(&h->outer, ih, 32);
    pspc_sha256_final(&h->outer, out);
    pspc_wipe(ih, sizeof ih);
}

void pspc_hmac_sha256(const uint8_t *key, size_t key_len, const uint8_t *msg, size_t msg_len, uint8_t out[32])
{
    hmac_ctx h;
    hmac_init(&h, key, key_len);
    pspc_sha256_update(&h.inner, msg, msg_len);
    hmac_final(&h, out);
}

int pspc_hkdf_sha256(const uint8_t *salt, size_t salt_len, const uint8_t *ikm, size_t ikm_len,
                     const uint8_t *info, size_t info_len, uint8_t *out, size_t out_len)
{
    static const uint8_t zero_salt[32] = {0};
    uint8_t prk[32], t[32];
    size_t done = 0, t_len = 0;
    uint8_t counter = 1;
    if (out_len > 255 * 32) return -1;
    if (salt == NULL) {
        salt = zero_salt;
        salt_len = sizeof zero_salt;
    }
    pspc_hmac_sha256(salt, salt_len, ikm, ikm_len, prk);
    while (done < out_len) {
        hmac_ctx h;
        size_t take;
        hmac_init(&h, prk, 32);
        pspc_sha256_update(&h.inner, t, t_len);
        pspc_sha256_update(&h.inner, info, info_len);
        pspc_sha256_update(&h.inner, &counter, 1);
        hmac_final(&h, t);
        t_len = 32;
        take = out_len - done < 32 ? out_len - done : 32;
        memcpy(out + done, t, take);
        done += take;
        counter++;
    }
    pspc_wipe(prk, sizeof prk);
    pspc_wipe(t, sizeof t);
    return 0;
}

/* ---------- ChaCha20 ---------- */

#define ROL32(x, n) (((x) << (n)) | ((x) >> (32 - (n))))
#define QR(a, b, c, d)                     \
    a += b; d ^= a; d = ROL32(d, 16);      \
    c += d; b ^= c; b = ROL32(b, 12);      \
    a += b; d ^= a; d = ROL32(d, 8);       \
    c += d; b ^= c; b = ROL32(b, 7)

static void chacha20_block(const uint8_t key[32], uint32_t counter, const uint8_t nonce[12], uint8_t out[64])
{
    uint32_t s[16], x[16];
    int i;
    s[0] = 0x61707865; s[1] = 0x3320646e; s[2] = 0x79622d32; s[3] = 0x6b206574;
    for (i = 0; i < 8; i++) s[4 + i] = ld32le(key + 4 * i);
    s[12] = counter;
    for (i = 0; i < 3; i++) s[13 + i] = ld32le(nonce + 4 * i);
    memcpy(x, s, sizeof s);
    for (i = 0; i < 10; i++) {
        QR(x[0], x[4], x[8], x[12]);
        QR(x[1], x[5], x[9], x[13]);
        QR(x[2], x[6], x[10], x[14]);
        QR(x[3], x[7], x[11], x[15]);
        QR(x[0], x[5], x[10], x[15]);
        QR(x[1], x[6], x[11], x[12]);
        QR(x[2], x[7], x[8], x[13]);
        QR(x[3], x[4], x[9], x[14]);
    }
    for (i = 0; i < 16; i++) st32le(out + 4 * i, x[i] + s[i]);
    pspc_wipe(x, sizeof x);
    pspc_wipe(s, sizeof s);
}

void pspc_chacha20_xor(const uint8_t key[32], uint32_t counter, const uint8_t nonce[12],
                       const uint8_t *in, uint8_t *out, size_t n)
{
    uint8_t ks[64];
    while (n > 0) {
        size_t take = n < 64 ? n : 64, i;
        chacha20_block(key, counter++, nonce, ks);
        for (i = 0; i < take; i++) out[i] = (uint8_t)(in[i] ^ ks[i]);
        in += take;
        out += take;
        n -= take;
    }
    pspc_wipe(ks, sizeof ks);
}

/* ---------- Poly1305 ---------- */

typedef struct {
    uint32_t r[5], h[5], pad[4];
    uint8_t buf[16];
    size_t fill;
} poly_ctx;

static void poly_init(poly_ctx *p, const uint8_t key[32])
{
    p->r[0] = ld32le(key + 0) & 0x3ffffff;
    p->r[1] = (ld32le(key + 3) >> 2) & 0x3ffff03;
    p->r[2] = (ld32le(key + 6) >> 4) & 0x3ffc0ff;
    p->r[3] = (ld32le(key + 9) >> 6) & 0x3f03fff;
    p->r[4] = (ld32le(key + 12) >> 8) & 0x00fffff;
    memset(p->h, 0, sizeof p->h);
    p->pad[0] = ld32le(key + 16);
    p->pad[1] = ld32le(key + 20);
    p->pad[2] = ld32le(key + 24);
    p->pad[3] = ld32le(key + 28);
    p->fill = 0;
}

static void poly_block(poly_ctx *p, const uint8_t *m, uint32_t hibit)
{
    const uint32_t r0 = p->r[0], r1 = p->r[1], r2 = p->r[2], r3 = p->r[3], r4 = p->r[4];
    const uint32_t s1 = r1 * 5, s2 = r2 * 5, s3 = r3 * 5, s4 = r4 * 5;
    uint32_t h0 = p->h[0], h1 = p->h[1], h2 = p->h[2], h3 = p->h[3], h4 = p->h[4], c;
    uint64_t d0, d1, d2, d3, d4;

    h0 += ld32le(m + 0) & 0x3ffffff;
    h1 += (ld32le(m + 3) >> 2) & 0x3ffffff;
    h2 += (ld32le(m + 6) >> 4) & 0x3ffffff;
    h3 += (ld32le(m + 9) >> 6) & 0x3ffffff;
    h4 += (ld32le(m + 12) >> 8) | hibit;

    d0 = (uint64_t)h0 * r0 + (uint64_t)h1 * s4 + (uint64_t)h2 * s3 + (uint64_t)h3 * s2 + (uint64_t)h4 * s1;
    d1 = (uint64_t)h0 * r1 + (uint64_t)h1 * r0 + (uint64_t)h2 * s4 + (uint64_t)h3 * s3 + (uint64_t)h4 * s2;
    d2 = (uint64_t)h0 * r2 + (uint64_t)h1 * r1 + (uint64_t)h2 * r0 + (uint64_t)h3 * s4 + (uint64_t)h4 * s3;
    d3 = (uint64_t)h0 * r3 + (uint64_t)h1 * r2 + (uint64_t)h2 * r1 + (uint64_t)h3 * r0 + (uint64_t)h4 * s4;
    d4 = (uint64_t)h0 * r4 + (uint64_t)h1 * r3 + (uint64_t)h2 * r2 + (uint64_t)h3 * r1 + (uint64_t)h4 * r0;

    c = (uint32_t)(d0 >> 26); h0 = (uint32_t)d0 & 0x3ffffff;
    d1 += c; c = (uint32_t)(d1 >> 26); h1 = (uint32_t)d1 & 0x3ffffff;
    d2 += c; c = (uint32_t)(d2 >> 26); h2 = (uint32_t)d2 & 0x3ffffff;
    d3 += c; c = (uint32_t)(d3 >> 26); h3 = (uint32_t)d3 & 0x3ffffff;
    d4 += c; c = (uint32_t)(d4 >> 26); h4 = (uint32_t)d4 & 0x3ffffff;
    h0 += c * 5; c = h0 >> 26; h0 &= 0x3ffffff;
    h1 += c;

    p->h[0] = h0; p->h[1] = h1; p->h[2] = h2; p->h[3] = h3; p->h[4] = h4;
}

static void poly_update(poly_ctx *p, const uint8_t *m, size_t n)
{
    while (n > 0) {
        size_t take = 16 - p->fill;
        if (take > n) take = n;
        memcpy(p->buf + p->fill, m, take);
        p->fill += take;
        m += take;
        n -= take;
        if (p->fill == 16) {
            poly_block(p, p->buf, 1u << 24);
            p->fill = 0;
        }
    }
}

static void poly_final(poly_ctx *p, uint8_t tag[16])
{
    uint32_t h0, h1, h2, h3, h4, c, g0, g1, g2, g3, g4, mask;
    uint64_t f;
    if (p->fill > 0) {
        p->buf[p->fill] = 1;
        memset(p->buf + p->fill + 1, 0, 16 - p->fill - 1);
        poly_block(p, p->buf, 0);
    }
    h0 = p->h[0]; h1 = p->h[1]; h2 = p->h[2]; h3 = p->h[3]; h4 = p->h[4];

    c = h1 >> 26; h1 &= 0x3ffffff;
    h2 += c; c = h2 >> 26; h2 &= 0x3ffffff;
    h3 += c; c = h3 >> 26; h3 &= 0x3ffffff;
    h4 += c; c = h4 >> 26; h4 &= 0x3ffffff;
    h0 += c * 5; c = h0 >> 26; h0 &= 0x3ffffff;
    h1 += c;

    g0 = h0 + 5; c = g0 >> 26; g0 &= 0x3ffffff;
    g1 = h1 + c; c = g1 >> 26; g1 &= 0x3ffffff;
    g2 = h2 + c; c = g2 >> 26; g2 &= 0x3ffffff;
    g3 = h3 + c; c = g3 >> 26; g3 &= 0x3ffffff;
    g4 = h4 + c - (1u << 26);

    /* h - p is negative (top bit set) when h < p; then keep h. */
    mask = (g4 >> 31) - 1;
    g0 &= mask; g1 &= mask; g2 &= mask; g3 &= mask; g4 &= mask;
    mask = ~mask;
    h0 = (h0 & mask) | g0;
    h1 = (h1 & mask) | g1;
    h2 = (h2 & mask) | g2;
    h3 = (h3 & mask) | g3;
    h4 = (h4 & mask) | g4;

    h0 = h0 | (h1 << 26);
    h1 = (h1 >> 6) | (h2 << 20);
    h2 = (h2 >> 12) | (h3 << 14);
    h3 = (h3 >> 18) | (h4 << 8);

    f = (uint64_t)h0 + p->pad[0]; h0 = (uint32_t)f;
    f = (uint64_t)h1 + p->pad[1] + (f >> 32); h1 = (uint32_t)f;
    f = (uint64_t)h2 + p->pad[2] + (f >> 32); h2 = (uint32_t)f;
    f = (uint64_t)h3 + p->pad[3] + (f >> 32); h3 = (uint32_t)f;

    st32le(tag + 0, h0);
    st32le(tag + 4, h1);
    st32le(tag + 8, h2);
    st32le(tag + 12, h3);
    pspc_wipe(p, sizeof *p);
}

void pspc_poly1305(const uint8_t key[32], const uint8_t *msg, size_t n, uint8_t tag[16])
{
    poly_ctx p;
    poly_init(&p, key);
    poly_update(&p, msg, n);
    poly_final(&p, tag);
}

/* ---------- AEAD (RFC 8439) ---------- */

static void aead_tag(const uint8_t key[32], const uint8_t nonce[12], const uint8_t *aad, size_t aad_len,
                     const uint8_t *ct, size_t ct_len, uint8_t tag[16])
{
    static const uint8_t zeros[16] = {0};
    uint8_t block0[64], lens[16];
    poly_ctx p;
    memset(block0, 0, sizeof block0);
    pspc_chacha20_xor(key, 0, nonce, block0, block0, 64);
    poly_init(&p, block0);
    poly_update(&p, aad, aad_len);
    poly_update(&p, zeros, (16 - aad_len % 16) % 16);
    poly_update(&p, ct, ct_len);
    poly_update(&p, zeros, (16 - ct_len % 16) % 16);
    st64le(lens, (uint64_t)aad_len);
    st64le(lens + 8, (uint64_t)ct_len);
    poly_update(&p, lens, 16);
    poly_final(&p, tag);
    pspc_wipe(block0, sizeof block0);
}

void pspc_aead_seal(const uint8_t key[32], const uint8_t nonce[12], const uint8_t *aad, size_t aad_len,
                    const uint8_t *pt, size_t len, uint8_t *out)
{
    pspc_chacha20_xor(key, 1, nonce, pt, out, len);
    aead_tag(key, nonce, aad, aad_len, out, len, out + len);
}

int pspc_aead_open(const uint8_t key[32], const uint8_t nonce[12], const uint8_t *aad, size_t aad_len,
                   const uint8_t *ct, size_t ct_len, uint8_t *out)
{
    uint8_t tag[16];
    size_t len;
    if (ct_len < PSPC_TAG_LEN) return -1;
    len = ct_len - PSPC_TAG_LEN;
    aead_tag(key, nonce, aad, aad_len, ct, len, tag);
    if (!pspc_ct_equal(tag, ct + len, PSPC_TAG_LEN)) {
        memset(out, 0, len);
        return -1;
    }
    pspc_chacha20_xor(key, 1, nonce, ct, out, len);
    return 0;
}

/* ---------- edwards25519 (TweetNaCl arithmetic) ---------- */

typedef int64_t gf[16];

static const gf gf0 = {0};
static const gf gf1 = {1};
static const gf D = {0x78a3, 0x1359, 0x4dca, 0x75eb, 0xd8ab, 0x4141, 0x0a4d, 0x0070,
                     0xe898, 0x7779, 0x4079, 0x8cc7, 0xfe73, 0x2b6f, 0x6cee, 0x5203};
static const gf D2 = {0xf159, 0x26b2, 0x9b94, 0xebd6, 0xb156, 0x8283, 0x149a, 0x00e0,
                      0xd130, 0xeef3, 0x80f2, 0x198e, 0xfce7, 0x56df, 0xd9dc, 0x2406};
static const gf BX = {0xd51a, 0x8f25, 0x2d60, 0xc956, 0xa7b2, 0x9525, 0xc760, 0x692c,
                      0xdc5c, 0xfdd6, 0xe231, 0xc0a4, 0x53fe, 0xcd6e, 0x36d3, 0x2169};
static const gf BY = {0x6658, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666,
                      0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666};
static const gf SQRTM1 = {0xa0b0, 0x4a0e, 0x1b27, 0xc4ee, 0xe478, 0xad2f, 0x1806, 0x2f43,
                          0xd7a7, 0x3dfb, 0x0099, 0x2b4d, 0xdf0b, 0x4fc1, 0x2480, 0x2b83};

static void set25519(gf r, const gf a)
{
    int i;
    for (i = 0; i < 16; i++) r[i] = a[i];
}

static void car25519(gf o)
{
    int i;
    int64_t c;
    for (i = 0; i < 16; i++) {
        o[i] += (int64_t)1 << 16;
        c = o[i] >> 16;
        o[(i + 1) * (i < 15)] += c - 1 + 37 * (c - 1) * (i == 15);
        o[i] -= c * 65536;
    }
}

static void sel25519(gf p, gf q, int b)
{
    int64_t t, c = ~((int64_t)b - 1);
    int i;
    for (i = 0; i < 16; i++) {
        t = c & (p[i] ^ q[i]);
        p[i] ^= t;
        q[i] ^= t;
    }
}

static void pack25519(uint8_t *o, const gf n)
{
    int i, j, b;
    gf m, t;
    set25519(t, n);
    car25519(t);
    car25519(t);
    car25519(t);
    for (j = 0; j < 2; j++) {
        m[0] = t[0] - 0xffed;
        for (i = 1; i < 15; i++) {
            m[i] = t[i] - 0xffff - ((m[i - 1] >> 16) & 1);
            m[i - 1] &= 0xffff;
        }
        m[15] = t[15] - 0x7fff - ((m[14] >> 16) & 1);
        b = (int)((m[15] >> 16) & 1);
        m[14] &= 0xffff;
        sel25519(t, m, 1 - b);
    }
    for (i = 0; i < 16; i++) {
        o[2 * i] = (uint8_t)(t[i] & 0xff);
        o[2 * i + 1] = (uint8_t)(t[i] >> 8);
    }
}

static int neq25519(const gf a, const gf b)
{
    uint8_t c[32], d[32];
    pack25519(c, a);
    pack25519(d, b);
    return !pspc_ct_equal(c, d, 32);
}

static uint8_t par25519(const gf a)
{
    uint8_t d[32];
    pack25519(d, a);
    return d[0] & 1;
}

static void unpack25519(gf o, const uint8_t *n)
{
    int i;
    for (i = 0; i < 16; i++) o[i] = n[2 * i] + ((int64_t)n[2 * i + 1] << 8);
    o[15] &= 0x7fff;
}

static void fadd(gf o, const gf a, const gf b)
{
    int i;
    for (i = 0; i < 16; i++) o[i] = a[i] + b[i];
}

static void fsub(gf o, const gf a, const gf b)
{
    int i;
    for (i = 0; i < 16; i++) o[i] = a[i] - b[i];
}

static void fmul(gf o, const gf a, const gf b)
{
    int64_t t[31];
    int i, j;
    for (i = 0; i < 31; i++) t[i] = 0;
    for (i = 0; i < 16; i++)
        for (j = 0; j < 16; j++) t[i + j] += a[i] * b[j];
    for (i = 0; i < 15; i++) t[i] += 38 * t[i + 16];
    for (i = 0; i < 16; i++) o[i] = t[i];
    car25519(o);
    car25519(o);
}

static void fsq(gf o, const gf a)
{
    fmul(o, a, a);
}

static void inv25519(gf o, const gf i)
{
    gf c;
    int a;
    set25519(c, i);
    for (a = 253; a >= 0; a--) {
        fsq(c, c);
        if (a != 2 && a != 4) fmul(c, c, i);
    }
    set25519(o, c);
}

static void pow2523(gf o, const gf i)
{
    gf c;
    int a;
    set25519(c, i);
    for (a = 250; a >= 0; a--) {
        fsq(c, c);
        if (a != 1) fmul(c, c, i);
    }
    set25519(o, c);
}

/* Extended coordinates (X, Y, Z, T). */
typedef gf ge[4];

static void ge_add(ge p, ge q)
{
    gf a, b, c, d, t, e, f, g, h;
    fsub(a, p[1], p[0]);
    fsub(t, q[1], q[0]);
    fmul(a, a, t);
    fadd(b, p[0], p[1]);
    fadd(t, q[0], q[1]);
    fmul(b, b, t);
    fmul(c, p[3], q[3]);
    fmul(c, c, D2);
    fmul(d, p[2], q[2]);
    fadd(d, d, d);
    fsub(e, b, a);
    fsub(f, d, c);
    fadd(g, d, c);
    fadd(h, b, a);
    fmul(p[0], e, f);
    fmul(p[1], h, g);
    fmul(p[2], g, f);
    fmul(p[3], e, h);
}

static void ge_cswap(ge p, ge q, int b)
{
    int i;
    for (i = 0; i < 4; i++) sel25519(p[i], q[i], b);
}

static void ge_copy(ge r, ge p)
{
    int i;
    for (i = 0; i < 4; i++) set25519(r[i], p[i]);
}

static void ge_neg(ge p)
{
    fsub(p[0], gf0, p[0]);
    fsub(p[3], gf0, p[3]);
}

static void ge_pack(uint8_t r[32], ge p)
{
    gf tx, ty, zi;
    inv25519(zi, p[2]);
    fmul(tx, p[0], zi);
    fmul(ty, p[1], zi);
    pack25519(r, ty);
    r[31] ^= (uint8_t)(par25519(tx) << 7);
}

/* r = s * q, constant-time ladder over all 256 bits of s. q is preserved. */
static void ge_scalarmult(ge r, ge q_in, const uint8_t s[32])
{
    ge q;
    int i;
    ge_copy(q, q_in);
    set25519(r[0], gf0);
    set25519(r[1], gf1);
    set25519(r[2], gf1);
    set25519(r[3], gf0);
    for (i = 255; i >= 0; --i) {
        int b = (s[i / 8] >> (i & 7)) & 1;
        ge_cswap(r, q, b);
        ge_add(q, r);
        ge_add(r, r);
        ge_cswap(r, q, b);
    }
}

static void ge_scalarbase(ge r, const uint8_t s[32])
{
    ge base;
    set25519(base[0], BX);
    set25519(base[1], BY);
    set25519(base[2], gf1);
    fmul(base[3], BX, BY);
    ge_scalarmult(r, base, s);
}

/* Decompresses an RFC 8032 point encoding. Returns 0 on success. */
static int ge_unpack(ge r, const uint8_t p[32])
{
    gf t, chk, num, den, den2, den4, den6;
    set25519(r[2], gf1);
    unpack25519(r[1], p);
    fsq(num, r[1]);
    fmul(den, num, D);
    fsub(num, num, r[2]);
    fadd(den, r[2], den);

    fsq(den2, den);
    fsq(den4, den2);
    fmul(den6, den4, den2);
    fmul(t, den6, num);
    fmul(t, t, den);

    pow2523(t, t);
    fmul(t, t, num);
    fmul(t, t, den);
    fmul(t, t, den);
    fmul(r[0], t, den);

    fsq(chk, r[0]);
    fmul(chk, chk, den);
    if (neq25519(chk, num)) fmul(r[0], r[0], SQRTM1);

    fsq(chk, r[0]);
    fmul(chk, chk, den);
    if (neq25519(chk, num)) return -1;

    if (par25519(r[0]) != (p[31] >> 7)) fsub(r[0], gf0, r[0]);

    fmul(r[3], r[0], r[1]);
    return 0;
}

/* ---------- scalars mod L ---------- */

static const int64_t L[32] = {0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7,
                              0xa2, 0xde, 0xf9, 0xde, 0x14, 0,    0,    0,    0,    0,    0,
                              0,    0,    0,    0,    0,    0,    0,    0,    0,    0x10};

static void modL(uint8_t *r, int64_t x[64])
{
    int64_t carry;
    int i, j;
    for (i = 63; i >= 32; --i) {
        carry = 0;
        for (j = i - 32; j < i - 12; ++j) {
            x[j] += carry - 16 * x[i] * L[j - (i - 32)];
            carry = (x[j] + 128) >> 8;
            x[j] -= carry * 256;
        }
        x[j] += carry;
        x[i] = 0;
    }
    carry = 0;
    for (j = 0; j < 32; j++) {
        x[j] += carry - (x[31] >> 4) * L[j];
        carry = x[j] >> 8;
        x[j] &= 255;
    }
    for (j = 0; j < 32; j++) x[j] -= carry * L[j];
    for (i = 0; i < 32; i++) {
        x[i + 1] += x[i] >> 8;
        r[i] = (uint8_t)(x[i] & 255);
    }
}

/* out = wide (64 bytes, little-endian) mod L. */
static void scalar_reduce(uint8_t out[32], const uint8_t wide[64])
{
    int64_t x[64];
    int i;
    for (i = 0; i < 64; i++) x[i] = wide[i];
    modL(out, x);
    pspc_wipe(x, sizeof x);
}

/* ---------- SPAKE2 pairing ---------- */

static const uint8_t SPAKE_M[32] = {0xd0, 0x48, 0x03, 0x2c, 0x6e, 0xa0, 0xb6, 0xd6, 0x97, 0xdd, 0xc2,
                                    0xe8, 0x6b, 0xda, 0x85, 0xa3, 0x3a, 0xda, 0xc9, 0x20, 0xf1, 0xbf,
                                    0x18, 0xe1, 0xb0, 0xc6, 0xd1, 0x66, 0xa5, 0xce, 0xcd, 0xaf};
static const uint8_t SPAKE_N[32] = {0xd3, 0xbf, 0xb5, 0x18, 0xf4, 0x4f, 0x34, 0x30, 0xf2, 0x9d, 0x0c,
                                    0x92, 0xaf, 0x50, 0x38, 0x65, 0xa1, 0xed, 0x32, 0x81, 0xdc, 0x69,
                                    0xb3, 0x5d, 0xd8, 0x68, 0xba, 0x85, 0xf8, 0x86, 0xc4, 0xab};

static int valid_pin(const char *pin)
{
    int i;
    for (i = 0; i < 6; i++)
        if (pin[i] < '0' || pin[i] > '9') return 0;
    return pin[6] == '\0';
}

int pspc_pair_start(pspc_pair_client *c, const char *pin, const uint8_t device_id[16], const uint8_t x_wide[64])
{
    uint8_t h[64];
    pspc_sha512_ctx s;
    ge xg, m, wm;
    if (!valid_pin(pin)) return PSPC_PAIR_BAD_PIN;

    pspc_sha512_init(&s);
    pspc_sha512_update(&s, "PSPC-PIN-v1", 11);
    pspc_sha512_update(&s, pin, 6);
    pspc_sha512_final(&s, h);
    scalar_reduce(c->w, h);
    scalar_reduce(c->x, x_wide);
    memcpy(c->device_id, device_id, 16);

    if (ge_unpack(m, SPAKE_M) != 0) return PSPC_PAIR_BAD_POINT;
    ge_scalarbase(xg, c->x);
    ge_scalarmult(wm, m, c->w);
    ge_add(xg, wm);
    ge_pack(c->pa, xg);
    pspc_wipe(h, sizeof h);
    return PSPC_PAIR_OK;
}

static void tt_part(pspc_sha512_ctx *s, uint8_t *tt, size_t *off, const uint8_t *part, size_t n)
{
    uint8_t len[8];
    st64le(len, (uint64_t)n);
    pspc_sha512_update(s, len, 8);
    pspc_sha512_update(s, part, n);
    memcpy(tt + *off, len, 8);
    memcpy(tt + *off + 8, part, n);
    *off += 8 + n;
}

int pspc_pair_finish(pspc_pair_client *c, const uint8_t host_id[16], const uint8_t pb[32], const uint8_t cb[32],
                     uint8_t ca_out[32], uint8_t key_out[32])
{
    static const uint8_t identity[32] = {1};
    uint8_t k[32], h[64], kc[64], expect_cb[32];
    uint8_t tt[6 * 8 + 2 * 16 + 4 * 32];
    size_t off = 0;
    pspc_sha512_ctx s;
    ge b, n, wn, kp;
    int i, rc = PSPC_PAIR_OK;

    if (ge_unpack(b, pb) != 0 || ge_unpack(n, SPAKE_N) != 0) return PSPC_PAIR_BAD_POINT;
    ge_scalarmult(wn, n, c->w);
    ge_neg(wn);
    ge_add(b, wn);
    ge_scalarmult(kp, b, c->x);
    for (i = 0; i < 3; i++) ge_add(kp, kp);
    ge_pack(k, kp);
    if (pspc_ct_equal(k, identity, 32)) return PSPC_PAIR_BAD_POINT;

    pspc_sha512_init(&s);
    tt_part(&s, tt, &off, c->device_id, 16);
    tt_part(&s, tt, &off, host_id, 16);
    tt_part(&s, tt, &off, c->pa, 32);
    tt_part(&s, tt, &off, pb, 32);
    tt_part(&s, tt, &off, k, 32);
    tt_part(&s, tt, &off, c->w, 32);
    pspc_sha512_final(&s, h); /* Ke = h[0..32], Ka = h[32..64] */

    pspc_hkdf_sha256(NULL, 0, h + 32, 32, (const uint8_t *)"ConfirmationKeys", 16, kc, 64);
    pspc_hmac_sha256(kc + 32, 32, tt, off, expect_cb);
    if (!pspc_ct_equal(expect_cb, cb, 32)) {
        rc = PSPC_PAIR_CONFIRM_FAILED;
    } else {
        pspc_hmac_sha256(kc, 32, tt, off, ca_out);
        pspc_hkdf_sha256(NULL, 0, h, 32, (const uint8_t *)"PSPC v1 pairing key", 19, key_out, 32);
    }
    pspc_wipe(k, sizeof k);
    pspc_wipe(h, sizeof h);
    pspc_wipe(kc, sizeof kc);
    pspc_wipe(tt, sizeof tt);
    pspc_wipe(c->x, sizeof c->x);
    return rc;
}

void pspc_session_keys(const uint8_t pairing_key[32], const uint8_t *hello, size_t hello_len,
                       const uint8_t *reply, size_t reply_len, uint8_t out[128])
{
    uint8_t th[32];
    pspc_sha256_ctx s;
    pspc_sha256_init(&s);
    pspc_sha256_update(&s, hello, hello_len);
    pspc_sha256_update(&s, reply, reply_len);
    pspc_sha256_final(&s, th);
    pspc_hkdf_sha256(th, 32, pairing_key, 32, (const uint8_t *)"PSPC v1 session keys", 20, out, 128);
}
