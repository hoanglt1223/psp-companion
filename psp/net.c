#include "net.h"

#include <arpa/inet.h>
#include <errno.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <pspkernel.h>
#include <pspthreadman.h>
#include <stdio.h>
#include <string.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <unistd.h>

#include "crypto.h"
#include "rng.h"

#define CONNECT_TIMEOUT_US (5 * 1000 * 1000ULL)
#define REPLY_TIMEOUT_US (10 * 1000 * 1000ULL)
#define PONG_TIMEOUT_US (10 * 1000 * 1000ULL)
#define REGISTER_INTERVAL_US (2 * 1000 * 1000ULL)
#define REGISTER_TRIES 5
#define DISCOVERY_INTERVAL_US (1000 * 1000ULL)
#define DISCOVERY_TRIES 5
#define TX_CAP 8192
#define SW_VERSION "0.1.0"

/* ---------- utilities ---------- */

static net_stats g_stats = {.rtt_ms = -1};
static uint64_t g_win_start, g_win_in, g_win_out;

uint64_t net_now_us(void)
{
    return (uint64_t)sceKernelGetSystemTimeWide();
}

static void count_in(size_t n)
{
    g_stats.total_in += n;
    g_win_in += n;
}

static void count_out(size_t n)
{
    g_stats.total_out += n;
    g_win_out += n;
}

void net_stats_tick(void)
{
    uint64_t now = net_now_us(), dt = now - g_win_start;
    if (g_win_start == 0) {
        g_win_start = now;
        return;
    }
    if (dt < 1000000ULL) return;
    g_stats.kbps_in = (float)g_win_in * 8.0f * 1000.0f / (float)dt;
    g_stats.kbps_out = (float)g_win_out * 8.0f * 1000.0f / (float)dt;
    g_win_in = g_win_out = 0;
    g_win_start = now;
}

const net_stats *net_get_stats(void)
{
    return &g_stats;
}

void net_ip_to_str(uint32_t ip, char out[16])
{
    const uint8_t *b = (const uint8_t *)&ip;
    snprintf(out, 16, "%u.%u.%u.%u", b[0], b[1], b[2], b[3]);
}

int net_parse_ip(const char *s, uint32_t *ip)
{
    uint8_t b[4];
    int i;
    for (i = 0; i < 4; i++) {
        unsigned v = 0, digits = 0;
        while (*s >= '0' && *s <= '9' && digits < 3) {
            v = v * 10 + (unsigned)(*s++ - '0');
            digits++;
        }
        if (digits == 0 || v > 255) return -1;
        b[i] = (uint8_t)v;
        if (i < 3 && *s++ != '.') return -1;
    }
    if (*s != '\0') return -1;
    memcpy(ip, b, 4);
    return 0;
}

static void set_err(char *err, size_t cap, const char *msg)
{
    if (err && cap) snprintf(err, cap, "%s", msg);
}

static int would_block(void)
{
    return errno == EAGAIN || errno == EWOULDBLOCK;
}

static void set_nonblocking(int fd)
{
    int one = 1;
    setsockopt(fd, SOL_SOCKET, SO_NONBLOCK, &one, sizeof one);
}

static struct sockaddr_in make_addr(uint32_t ip, uint16_t port)
{
    struct sockaddr_in a;
    memset(&a, 0, sizeof a);
    a.sin_len = sizeof a;
    a.sin_family = AF_INET;
    a.sin_port = htons(port);
    a.sin_addr.s_addr = ip;
    return a;
}

static int tcp_connect(uint32_t ip, uint16_t port)
{
    struct sockaddr_in a = make_addr(ip, port);
    uint64_t deadline = net_now_us() + CONNECT_TIMEOUT_US;
    int one = 1, fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) return -1;
    set_nonblocking(fd);
    setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &one, sizeof one);
    if (connect(fd, (struct sockaddr *)&a, sizeof a) == 0) return fd;
    if (errno != EINPROGRESS && errno != EALREADY && !would_block()) {
        close(fd);
        return -1;
    }
    while (net_now_us() < deadline) {
        fd_set w;
        struct timeval tv = {0, 50 * 1000};
        FD_ZERO(&w);
        FD_SET(fd, &w);
        if (select(fd + 1, NULL, &w, NULL, &tv) > 0) {
            int so_err = -1;
            socklen_t len = sizeof so_err;
            getsockopt(fd, SOL_SOCKET, SO_ERROR, &so_err, &len);
            if (so_err == 0) return fd;
            break;
        }
    }
    close(fd);
    return -1;
}

static int send_all(int fd, const uint8_t *p, size_t n, uint64_t deadline)
{
    while (n > 0) {
        int sent = (int)send(fd, p, n, 0);
        if (sent > 0) {
            p += sent;
            n -= (size_t)sent;
        } else if (sent < 0 && would_block() && net_now_us() < deadline) {
            sceKernelDelayThread(2000);
        } else {
            return -1;
        }
    }
    return 0;
}

static int recv_exact(int fd, uint8_t *p, size_t n, uint64_t deadline)
{
    while (n > 0) {
        int got = (int)recv(fd, p, n, 0);
        if (got > 0) {
            p += got;
            n -= (size_t)got;
        } else if (got < 0 && would_block() && net_now_us() < deadline) {
            sceKernelDelayThread(5000);
        } else {
            return -1;
        }
    }
    return 0;
}

static int send_clear_frame(int fd, const uint8_t *body, size_t len)
{
    uint8_t frame[2 + PSPC_HANDSHAKE_MAX];
    pspc_put16(frame, (uint16_t)len);
    memcpy(frame + 2, body, len);
    if (send_all(fd, frame, len + 2, net_now_us() + REPLY_TIMEOUT_US) != 0) return -1;
    count_out(len + 2 + PSPC_TCP_OVERHEAD);
    return 0;
}

/* Reads one frame body of at most cap bytes. Returns its length or -1. */
static int recv_frame(int fd, uint8_t *body, size_t cap, uint64_t deadline)
{
    uint8_t hdr[2];
    size_t len;
    if (recv_exact(fd, hdr, 2, deadline) != 0) return -1;
    len = pspc_get16(hdr);
    if (len == 0 || len > cap) return -1;
    if (recv_exact(fd, body, len, deadline) != 0) return -1;
    count_in(len + 2 + PSPC_TCP_OVERHEAD);
    return (int)len;
}

/* ---------- discovery ---------- */

static int g_disc_fd = -1;
static int g_disc_sent;
static uint64_t g_disc_next;
static uint8_t g_disc_nonces[DISCOVERY_TRIES][8];
static net_host g_hosts[NET_MAX_HOSTS];
static int g_nhosts;

int net_discovery_start(void)
{
    int one = 1;
    net_discovery_stop();
    g_disc_fd = socket(AF_INET, SOCK_DGRAM, 0);
    if (g_disc_fd < 0) return -1;
    set_nonblocking(g_disc_fd);
    setsockopt(g_disc_fd, SOL_SOCKET, SO_BROADCAST, &one, sizeof one);
    g_disc_sent = 0;
    g_disc_next = net_now_us();
    g_nhosts = 0;
    return 0;
}

void net_discovery_stop(void)
{
    if (g_disc_fd >= 0) close(g_disc_fd);
    g_disc_fd = -1;
}

int net_discovery_active(void)
{
    return g_disc_fd >= 0 && (g_disc_sent < DISCOVERY_TRIES || net_now_us() < g_disc_next);
}

int net_discovery_count(void)
{
    return g_nhosts;
}

const net_host *net_discovery_host(int i)
{
    return (i >= 0 && i < g_nhosts) ? &g_hosts[i] : NULL;
}

static void add_host(uint32_t ip, const pspc_announce *a)
{
    int i;
    for (i = 0; i < g_nhosts; i++) {
        if (memcmp(g_hosts[i].a.host_id, a->host_id, 16) == 0) {
            g_hosts[i].ip = ip;
            g_hosts[i].a = *a;
            return;
        }
    }
    if (g_nhosts < NET_MAX_HOSTS) {
        g_hosts[g_nhosts].ip = ip;
        g_hosts[g_nhosts].a = *a;
        g_nhosts++;
    }
}

void net_discovery_poll(void)
{
    uint8_t buf[128];
    if (g_disc_fd < 0) return;
    if (g_disc_sent < DISCOVERY_TRIES && net_now_us() >= g_disc_next) {
        struct sockaddr_in to = make_addr(htonl(INADDR_BROADCAST), PSPC_PORT_DISCOVERY);
        uint8_t req[PSPC_DISCOVER_LEN];
        rng_bytes(g_disc_nonces[g_disc_sent], 8);
        pspc_build_discover(req, g_disc_nonces[g_disc_sent]);
        if (sendto(g_disc_fd, req, sizeof req, 0, (struct sockaddr *)&to, sizeof to) == (int)sizeof req)
            count_out(sizeof req + PSPC_UDP_OVERHEAD);
        g_disc_sent++;
        g_disc_next = net_now_us() + DISCOVERY_INTERVAL_US;
    }
    for (;;) {
        struct sockaddr_in from;
        socklen_t from_len = sizeof from;
        pspc_announce a;
        int i, n = (int)recvfrom(g_disc_fd, buf, sizeof buf, 0, (struct sockaddr *)&from, &from_len);
        if (n <= 0) break;
        count_in((size_t)n + PSPC_UDP_OVERHEAD);
        if (pspc_parse_announce(buf, (size_t)n, &a) != PSPC_OK) continue;
        for (i = 0; i < g_disc_sent; i++) {
            if (memcmp(a.nonce, g_disc_nonces[i], 8) == 0) {
                add_host(from.sin_addr.s_addr, &a);
                break;
            }
        }
    }
}

/* ---------- pairing ---------- */

static const char *status_text(uint8_t st)
{
    switch (st) {
    case PSPC_ST_UNKNOWN: return "Pairing is not open. Choose 'Pair new PSP' on the PC.";
    case PSPC_ST_VERSION: return "The PC runs an incompatible version.";
    case PSPC_ST_TOO_MANY_ATTEMPTS: return "Too many wrong PINs. Show a new PIN on the PC.";
    case PSPC_ST_WRONG_PIN: return "Wrong PIN.";
    case PSPC_ST_BUSY: return "The PC could not save the pairing. Try again.";
    default: return "The PC refused the request.";
    }
}

int net_pair(uint32_t ip, uint16_t port, const char *host_name, const char *pin, const char *device_name,
             pairing_record *rec, char *err, size_t err_cap)
{
    pspc_pair_client client;
    uint8_t x_wide[64], device_id[16], body[PSPC_HANDSHAKE_MAX], host_id[16], pb[32], cb[32], ca[32], key[32];
    uint8_t status = 0xFF;
    int fd, len, rc = NET_ERR;

    rng_bytes(device_id, sizeof device_id);
    rng_bytes(x_wide, sizeof x_wide);
    if (pspc_pair_start(&client, pin, device_id, x_wide) != PSPC_PAIR_OK) {
        set_err(err, err_cap, "The PIN must be six digits.");
        return NET_ERR;
    }
    pspc_wipe(x_wide, sizeof x_wide);

    fd = tcp_connect(ip, port);
    if (fd < 0) {
        set_err(err, err_cap, "Cannot reach the PC (check the IP and firewall).");
        goto out;
    }
    len = (int)pspc_build_pair_request(body, device_id, client.pa, device_name);
    if (send_clear_frame(fd, body, (size_t)len) != 0 ||
        (len = recv_frame(fd, body, sizeof body, net_now_us() + REPLY_TIMEOUT_US)) < 0) {
        set_err(err, err_cap, "The PC did not answer the pairing request.");
        goto out;
    }
    rc = pspc_parse_pair_challenge(body, (size_t)len, &status, host_id, pb, cb);
    if (rc == PSPC_EVERSION) {
        set_err(err, err_cap, status_text(PSPC_ST_VERSION));
        rc = NET_ERR_VERSION;
        goto out;
    }
    if (rc != PSPC_OK) {
        rc = NET_ERR;
        set_err(err, err_cap, "The PC sent a malformed reply.");
        goto out;
    }
    rc = NET_ERR;
    if (status != PSPC_ST_OK) {
        set_err(err, err_cap, status_text(status));
        goto out;
    }
    switch (pspc_pair_finish(&client, host_id, pb, cb, ca, key)) {
    case PSPC_PAIR_OK:
        break;
    case PSPC_PAIR_CONFIRM_FAILED:
        set_err(err, err_cap, "Wrong PIN.");
        goto out;
    default:
        set_err(err, err_cap, "The PC sent invalid pairing data.");
        goto out;
    }
    len = (int)pspc_build_pair_confirm(body, ca);
    if (send_clear_frame(fd, body, (size_t)len) != 0 ||
        (len = recv_frame(fd, body, sizeof body, net_now_us() + REPLY_TIMEOUT_US)) < 0 ||
        pspc_parse_pair_result(body, (size_t)len, &status) != PSPC_OK) {
        set_err(err, err_cap, "The PC did not confirm the pairing.");
        goto out;
    }
    if (status != PSPC_ST_OK) {
        set_err(err, err_cap, status_text(status));
        goto out;
    }

    memset(rec, 0, sizeof *rec);
    memcpy(rec->host_id, host_id, 16);
    memcpy(rec->device_id, device_id, 16);
    memcpy(rec->key, key, 32);
    rec->host_ip = ip;
    rec->control_port = port;
    snprintf(rec->host_name, sizeof rec->host_name, "%s", host_name);
    if (storage_save(rec) != 0) {
        set_err(err, err_cap, "Paired, but saving to the Memory Stick failed.");
        goto out;
    }
    rc = NET_OK;
out:
    if (fd >= 0) close(fd);
    pspc_wipe(&client, sizeof client);
    pspc_wipe(key, sizeof key);
    return rc;
}

/* ---------- session ---------- */

static struct {
    int fd;
    int udp;
    uint8_t keys[128]; /* tcp_c2s, tcp_s2c, udp_c2s, udp_s2c */
    uint64_t tx_ctr, rx_ctr, udp_seq;
    uint64_t udp_highest, udp_bitmap;
    int udp_seen;
    uint32_t session_tag;
    uint32_t host_ip;
    uint16_t audio_port;
    uint8_t rx[2 + PSPC_MAX_FRAME_BODY];
    size_t rx_len;
    uint8_t tx[TX_CAP];
    size_t tx_len;
    uint32_t ping_id;
    int ping_outstanding;
    uint64_t ping_sent_us, next_ping_us, last_manual_ping_us;
    int register_tries;
    uint64_t next_register_us;
} S = {.fd = -1, .udp = -1};

static uint8_t g_plain[PSPC_MAX_MESSAGE];
static net_message_fn g_on_message;

void net_set_message_handler(net_message_fn fn)
{
    g_on_message = fn;
}

#define KEY_TCP_C2S (S.keys)
#define KEY_TCP_S2C (S.keys + 32)
#define KEY_UDP_C2S (S.keys + 64)
#define KEY_UDP_S2C (S.keys + 96)

static int queue_msg(const uint8_t *pt, size_t len)
{
    uint8_t nonce[12], *out;
    size_t body = len + PSPC_TAG_LEN;
    if (S.tx_len + 2 + body > TX_CAP) return -1;
    out = S.tx + S.tx_len;
    pspc_put16(out, (uint16_t)body);
    pspc_nonce(S.tx_ctr++, nonce);
    pspc_aead_seal(KEY_TCP_C2S, nonce, out, 2, pt, len, out + 2);
    S.tx_len += 2 + body;
    count_out(2 + body + PSPC_TCP_OVERHEAD);
    return 0;
}

static int flush_tx(void)
{
    while (S.tx_len > 0) {
        int n = (int)send(S.fd, S.tx, S.tx_len, 0);
        if (n > 0) {
            memmove(S.tx, S.tx + n, S.tx_len - (size_t)n);
            S.tx_len -= (size_t)n;
        } else if (n < 0 && would_block()) {
            return 0;
        } else {
            return -1;
        }
    }
    return 0;
}

static void send_datagram(const uint8_t *pt, size_t len)
{
    uint8_t dgram[PSPC_MAX_DATAGRAM], nonce[12];
    struct sockaddr_in to = make_addr(S.host_ip, S.audio_port);
    size_t total = PSPC_UDP_HEADER + len + PSPC_TAG_LEN;
    if (S.udp < 0 || total > sizeof dgram) return;
    dgram[0] = 0x01;
    pspc_put32(dgram + 1, S.session_tag);
    pspc_put64(dgram + 5, S.udp_seq);
    pspc_nonce(S.udp_seq++, nonce);
    pspc_aead_seal(KEY_UDP_C2S, nonce, dgram, PSPC_UDP_HEADER, pt, len, dgram + PSPC_UDP_HEADER);
    if (sendto(S.udp, dgram, total, 0, (struct sockaddr *)&to, sizeof to) == (int)total)
        count_out(total + PSPC_UDP_OVERHEAD);
}

static int udp_replay_ok(uint64_t seq)
{
    if (!S.udp_seen || seq > S.udp_highest) return 1;
    return (S.udp_highest - seq) < 64 && !(S.udp_bitmap & (1ULL << (S.udp_highest - seq)));
}

static void udp_replay_accept(uint64_t seq)
{
    if (!S.udp_seen) {
        S.udp_seen = 1;
        S.udp_highest = seq;
        S.udp_bitmap = 1;
    } else if (seq > S.udp_highest) {
        uint64_t shift = seq - S.udp_highest;
        S.udp_bitmap = shift >= 64 ? 0 : S.udp_bitmap << shift;
        S.udp_bitmap |= 1;
        S.udp_highest = seq;
    } else {
        S.udp_bitmap |= 1ULL << (S.udp_highest - seq);
    }
}

static void poll_udp(void)
{
    static uint8_t dgram[2048];
    uint8_t nonce[12];
    for (;;) {
        struct sockaddr_in from;
        socklen_t from_len = sizeof from;
        uint64_t seq;
        size_t ct_len;
        int n = (int)recvfrom(S.udp, dgram, sizeof dgram, 0, (struct sockaddr *)&from, &from_len);
        if (n <= 0) return;
        count_in((size_t)n + PSPC_UDP_OVERHEAD);
        if (from.sin_addr.s_addr != S.host_ip || n < PSPC_UDP_HEADER + PSPC_TAG_LEN || n > PSPC_MAX_DATAGRAM ||
            dgram[0] != 0x01 || pspc_get32(dgram + 1) != S.session_tag) {
            g_stats.udp_dropped++;
            continue;
        }
        seq = pspc_get64(dgram + 5);
        ct_len = (size_t)n - PSPC_UDP_HEADER;
        pspc_nonce(seq, nonce);
        if (!udp_replay_ok(seq) ||
            pspc_aead_open(KEY_UDP_S2C, nonce, dgram, PSPC_UDP_HEADER, dgram + PSPC_UDP_HEADER, ct_len, g_plain) != 0 ||
            g_plain[0] != PSPC_MSG_AUDIO_PACKET) {
            g_stats.udp_dropped++;
            continue;
        }
        udp_replay_accept(seq);
        /* Playback arrives with the PC speaker phase; for now packets are only counted. */
        g_stats.udp_packets++;
    }
}

static void send_ping(uint64_t now)
{
    uint8_t msg[16];
    size_t len = pspc_build_ping(msg, ++S.ping_id, now);
    if (queue_msg(msg, len) != 0) return;
    S.ping_outstanding = 1;
    S.ping_sent_us = now;
    S.next_ping_us = now + (uint64_t)g_stats.keepalive_s * 1000000ULL;
    g_stats.pings++;
}

/* Returns NET_OK to continue or NET_ERR to drop the session. */
static int handle_message(const uint8_t *m, size_t len, uint64_t now)
{
    uint32_t id;
    uint64_t t;
    uint8_t reason;
    uint16_t code;
    switch (m[0]) {
    case PSPC_MSG_PONG:
        if (pspc_parse_pong(m, len, &id, &t) != PSPC_OK) return NET_ERR;
        if (id == S.ping_id && S.ping_outstanding) {
            g_stats.rtt_ms = (int)((now - t) / 1000);
            S.ping_outstanding = 0;
            g_stats.pongs++;
        }
        return NET_OK;
    case PSPC_MSG_AUDIO_REGISTER_ACK:
        g_stats.udp_registered = 1;
        return NET_OK;
    case PSPC_MSG_BYE:
        if (pspc_parse_bye(m, len, &reason) != PSPC_OK) return NET_ERR;
        snprintf(g_stats.status, sizeof g_stats.status, "%s",
                 reason == PSPC_BYE_SHUTDOWN   ? "The PC app was closed."
                 : reason == PSPC_BYE_REPLACED ? "Another session from this PSP took over."
                 : reason == PSPC_BYE_UNPAIRED ? "The PC forgot this PSP. Pair again."
                                               : "The PC ended the session.");
        return reason == PSPC_BYE_UNPAIRED ? NET_ERR_UNPAIRED : NET_ERR;
    case PSPC_MSG_ERROR:
        if (pspc_parse_error(m, len, &code, g_stats.status, sizeof g_stats.status) != PSPC_OK) return NET_ERR;
        return NET_OK;
    case PSPC_MSG_HELLO_ACK:
        snprintf(g_stats.status, sizeof g_stats.status, "Protocol error: duplicate HelloAck.");
        return NET_ERR;
    default:
        /* Feature messages go to the app; unknown types are ignored by design. */
        if (g_on_message) g_on_message(m, len);
        return NET_OK;
    }
}

static void session_reset(void)
{
    if (S.fd >= 0) close(S.fd);
    if (S.udp >= 0) close(S.udp);
    pspc_wipe(&S, sizeof S);
    S.fd = -1;
    S.udp = -1;
    g_stats.connected = 0;
    g_stats.udp_registered = 0;
}

int net_session_open(const pairing_record *rec, const char *device_name, char *err, size_t err_cap)
{
    uint8_t hello[PSPC_HANDSHAKE_MAX], reply[PSPC_HANDSHAKE_MAX], nonce[16], host_id[16], server_nonce[16];
    uint8_t msg[PSPC_SMALL_MSG_MAX], n12[12], status = 0xFF;
    pspc_hello_ack ack;
    size_t hello_len, msg_len;
    int reply_len, body_len, rc;

    session_reset();
    S.fd = tcp_connect(rec->host_ip, rec->control_port);
    if (S.fd < 0) {
        set_err(err, err_cap, "Cannot reach the PC.");
        return NET_ERR;
    }
    rng_bytes(nonce, sizeof nonce);
    hello_len = pspc_build_session_hello(hello, rec->device_id, nonce);
    if (send_clear_frame(S.fd, hello, hello_len) != 0 ||
        (reply_len = recv_frame(S.fd, reply, sizeof reply, net_now_us() + REPLY_TIMEOUT_US)) < 0) {
        set_err(err, err_cap, "The PC did not answer.");
        goto fail;
    }
    rc = pspc_parse_session_reply(reply, (size_t)reply_len, &status, host_id, server_nonce);
    if (rc == PSPC_EVERSION || (rc == PSPC_OK && status == PSPC_ST_VERSION)) {
        set_err(err, err_cap, "The PC runs an incompatible version.");
        session_reset();
        return NET_ERR_VERSION;
    }
    if (rc != PSPC_OK) {
        set_err(err, err_cap, "The PC sent a malformed reply.");
        goto fail;
    }
    if (status == PSPC_ST_UNKNOWN || memcmp(host_id, rec->host_id, 16) != 0) {
        set_err(err, err_cap, "This PC does not know this PSP. Pair again.");
        session_reset();
        return NET_ERR_UNPAIRED;
    }
    if (status != PSPC_ST_OK) {
        set_err(err, err_cap, status_text(status));
        goto fail;
    }

    pspc_session_keys(rec->key, hello, hello_len, reply, (size_t)reply_len, S.keys);
    msg_len = pspc_build_hello(msg, 0, SW_VERSION, device_name);
    if (queue_msg(msg, msg_len) != 0 || send_all(S.fd, S.tx, S.tx_len, net_now_us() + REPLY_TIMEOUT_US) != 0) {
        set_err(err, err_cap, "Sending Hello failed.");
        goto fail;
    }
    S.tx_len = 0;
    body_len = recv_frame(S.fd, S.rx, PSPC_MAX_FRAME_BODY, net_now_us() + REPLY_TIMEOUT_US);
    if (body_len < PSPC_TAG_LEN + 1) {
        set_err(err, err_cap, "The PC closed the session (wrong key?). Pair again if this repeats.");
        goto fail;
    }
    pspc_nonce(S.rx_ctr++, n12);
    {
        uint8_t aad[2];
        pspc_put16(aad, (uint16_t)body_len);
        if (pspc_aead_open(KEY_TCP_S2C, n12, aad, 2, S.rx, (size_t)body_len, g_plain) != 0) {
            set_err(err, err_cap, "This is not the paired PC (authentication failed).");
            goto fail;
        }
    }
    if (pspc_parse_hello_ack(g_plain, (size_t)body_len - PSPC_TAG_LEN, &ack) != PSPC_OK) {
        set_err(err, err_cap, "The PC sent a malformed HelloAck.");
        goto fail;
    }

    S.session_tag = ack.session_tag;
    S.host_ip = rec->host_ip;
    S.audio_port = ack.audio_port;
    g_stats.keepalive_s = ack.keepalive_s < 15 ? 15 : ack.keepalive_s;
    g_stats.budget_kbps = ack.budget_kbps;
    snprintf(g_stats.host_name, sizeof g_stats.host_name, "%s", ack.host_name);
    net_ip_to_str(rec->host_ip, g_stats.host_ip);
    g_stats.connected = 1;
    g_stats.udp_registered = 0;
    g_stats.rtt_ms = -1;
    g_stats.status[0] = '\0';
    g_stats.connected_since_us = net_now_us();
    S.next_ping_us = g_stats.connected_since_us + (uint64_t)g_stats.keepalive_s * 1000000ULL;

    S.udp = socket(AF_INET, SOCK_DGRAM, 0);
    if (S.udp >= 0) {
        set_nonblocking(S.udp);
        S.next_register_us = net_now_us();
    }
    rng_mix(server_nonce, sizeof server_nonce);
    return NET_OK;

fail:
    session_reset();
    return NET_ERR;
}

int net_session_poll(void)
{
    uint64_t now = net_now_us();
    uint8_t n12[12];
    int rc = NET_OK;
    if (S.fd < 0) return NET_ERR;

    for (;;) {
        int n = (int)recv(S.fd, S.rx + S.rx_len, sizeof S.rx - S.rx_len, 0);
        if (n > 0) {
            S.rx_len += (size_t)n;
            if (S.rx_len == sizeof S.rx) break;
        } else if (n < 0 && would_block()) {
            break;
        } else {
            snprintf(g_stats.status, sizeof g_stats.status, "Connection to the PC was lost.");
            rc = NET_ERR;
            goto done;
        }
    }

    while (S.rx_len >= 2) {
        size_t body = pspc_get16(S.rx);
        if (body < PSPC_TAG_LEN + 1 || body > PSPC_MAX_FRAME_BODY) {
            snprintf(g_stats.status, sizeof g_stats.status, "Protocol error: bad frame length.");
            rc = NET_ERR;
            goto done;
        }
        if (S.rx_len < 2 + body) break;
        count_in(2 + body + PSPC_TCP_OVERHEAD);
        pspc_nonce(S.rx_ctr++, n12);
        if (pspc_aead_open(KEY_TCP_S2C, n12, S.rx, 2, S.rx + 2, body, g_plain) != 0) {
            snprintf(g_stats.status, sizeof g_stats.status, "Protocol error: decryption failed.");
            rc = NET_ERR;
            goto done;
        }
        rc = handle_message(g_plain, body - PSPC_TAG_LEN, now);
        memmove(S.rx, S.rx + 2 + body, S.rx_len - 2 - body);
        S.rx_len -= 2 + body;
        if (rc != NET_OK) goto done;
    }

    if (S.udp >= 0) poll_udp();

    if (S.ping_outstanding && now - S.ping_sent_us > PONG_TIMEOUT_US) {
        snprintf(g_stats.status, sizeof g_stats.status, "No reply from the PC for 10 s.");
        rc = NET_ERR;
        goto done;
    }
    if (!S.ping_outstanding && now >= S.next_ping_us) send_ping(now);

    if (!g_stats.udp_registered && S.udp >= 0 && S.register_tries < REGISTER_TRIES && now >= S.next_register_us) {
        uint8_t m[4];
        send_datagram(m, pspc_build_audio_register(m));
        S.register_tries++;
        S.next_register_us = now + REGISTER_INTERVAL_US;
    }

    if (flush_tx() != 0) {
        snprintf(g_stats.status, sizeof g_stats.status, "Connection to the PC was lost.");
        rc = NET_ERR;
    }
done:
    if (rc != NET_OK) session_reset();
    return rc;
}

void net_session_ping_now(void)
{
    uint64_t now = net_now_us();
    if (S.fd < 0 || S.ping_outstanding || now - S.last_manual_ping_us < 1000000ULL) return;
    S.last_manual_ping_us = now;
    send_ping(now);
}

void net_session_close(uint8_t bye_reason)
{
    uint8_t m[4];
    if (S.fd >= 0) {
        if (queue_msg(m, pspc_build_bye(m, bye_reason)) == 0)
            send_all(S.fd, S.tx, S.tx_len, net_now_us() + 300 * 1000ULL);
    }
    session_reset();
}

int net_send(const uint8_t *msg, size_t len)
{
    if (S.fd < 0) return -1;
    return queue_msg(msg, len);
}
