/* Portable crypto for the PSP client: SHA-2, HMAC/HKDF, ChaCha20-Poly1305,
 * SPAKE2 pairing on edwards25519, and the session key schedule.
 * Pure C99 with no PSP headers, so the host test harness can compile it too. */
#ifndef PSPC_CRYPTO_H
#define PSPC_CRYPTO_H

#include <stddef.h>
#include <stdint.h>

#define PSPC_TAG_LEN 16

typedef struct {
    uint32_t h[8];
    uint64_t len;
    uint8_t buf[64];
    size_t fill;
} pspc_sha256_ctx;

typedef struct {
    uint64_t h[8];
    uint64_t len;
    uint8_t buf[128];
    size_t fill;
} pspc_sha512_ctx;

void pspc_sha256_init(pspc_sha256_ctx *c);
void pspc_sha256_update(pspc_sha256_ctx *c, const void *data, size_t n);
void pspc_sha256_final(pspc_sha256_ctx *c, uint8_t out[32]);
void pspc_sha256(const void *data, size_t n, uint8_t out[32]);

void pspc_sha512_init(pspc_sha512_ctx *c);
void pspc_sha512_update(pspc_sha512_ctx *c, const void *data, size_t n);
void pspc_sha512_final(pspc_sha512_ctx *c, uint8_t out[64]);
void pspc_sha512(const void *data, size_t n, uint8_t out[64]);

void pspc_hmac_sha256(const uint8_t *key, size_t key_len, const uint8_t *msg, size_t msg_len, uint8_t out[32]);
/* Returns 0 on success, -1 if out_len > 255 * 32. salt may be NULL. */
int pspc_hkdf_sha256(const uint8_t *salt, size_t salt_len, const uint8_t *ikm, size_t ikm_len,
                     const uint8_t *info, size_t info_len, uint8_t *out, size_t out_len);

void pspc_chacha20_xor(const uint8_t key[32], uint32_t counter, const uint8_t nonce[12],
                       const uint8_t *in, uint8_t *out, size_t n);
void pspc_poly1305(const uint8_t key[32], const uint8_t *msg, size_t n, uint8_t tag[16]);

/* RFC 8439 AEAD. seal writes len + 16 bytes; open reads ct_len >= 16 bytes and
 * returns 0 on success, -1 on authentication failure (out is then zeroed). */
void pspc_aead_seal(const uint8_t key[32], const uint8_t nonce[12], const uint8_t *aad, size_t aad_len,
                    const uint8_t *pt, size_t len, uint8_t *out);
int pspc_aead_open(const uint8_t key[32], const uint8_t nonce[12], const uint8_t *aad, size_t aad_len,
                   const uint8_t *ct, size_t ct_len, uint8_t *out);

/* Protocol nonce: 4 zero bytes then the little-endian 64-bit counter. */
void pspc_nonce(uint64_t counter, uint8_t out[12]);

int pspc_ct_equal(const uint8_t *a, const uint8_t *b, size_t n);
void pspc_wipe(void *p, size_t n);

/* SPAKE2 client (party A). */
typedef struct {
    uint8_t x[32];
    uint8_t w[32];
    uint8_t device_id[16];
    uint8_t pa[32];
} pspc_pair_client;

#define PSPC_PAIR_OK 0
#define PSPC_PAIR_BAD_PIN (-1)
#define PSPC_PAIR_BAD_POINT (-2)
#define PSPC_PAIR_CONFIRM_FAILED (-3)

/* pin: six ASCII digits. x_wide: 64 random bytes. */
int pspc_pair_start(pspc_pair_client *c, const char *pin, const uint8_t device_id[16], const uint8_t x_wide[64]);
/* On success writes cA (to send) and the pairing key (to store). */
int pspc_pair_finish(pspc_pair_client *c, const uint8_t host_id[16], const uint8_t pb[32], const uint8_t cb[32],
                     uint8_t ca_out[32], uint8_t key_out[32]);

/* Session keys: out = tcp_c2s || tcp_s2c || udp_c2s || udp_s2c (32 bytes each). */
void pspc_session_keys(const uint8_t pairing_key[32], const uint8_t *hello, size_t hello_len,
                       const uint8_t *reply, size_t reply_len, uint8_t out[128]);

#endif
