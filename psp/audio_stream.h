/* The one audio stream the host plays on this PSP (PC speaker or voice reply): jitter buffer,
 * Media Engine decoding, concealment, and output on its own thread. */
#ifndef PSPC_AUDIO_STREAM_H
#define PSPC_AUDIO_STREAM_H

#include <stddef.h>
#include <stdint.h>

typedef struct {
    int active;
    uint8_t kind; /* PSPC_AUDIO_KIND_* */
    int speaker_wanted;
    int decoder_error; /* < 0 when the Media Engine could not be set up */
    int volume_pct;
    uint32_t depth_ms, target_ms, played, lost, late, underruns;
} audio_status;

/* Starts the player thread (idle until a stream starts). Returns < 0 on failure. */
int audio_init(void);
/* A new session: stops playback and forgets the speaker request. */
void audio_reset(void);
/* AudioStreamStart and AudioStreamStop; returns 1 if handled. */
int audio_on_message(const uint8_t *msg, size_t len);
/* AudioPacket from the UDP channel. */
void audio_on_datagram(const uint8_t *msg, size_t len);

void audio_set_speaker(int on);
/* Stops whatever plays now and tells the host (L+R during a reply). */
void audio_stop_current(void);
void audio_volume_step(int delta_pct);
void audio_get_status(audio_status *out);

#endif
