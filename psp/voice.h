/* Push-to-talk recording from the built-in microphone: 11025 Hz mono, IMA ADPCM, streamed to the host
 * while R is held, capped at 60 seconds. */
#ifndef PSPC_VOICE_H
#define PSPC_VOICE_H

#include <stddef.h>
#include <stdint.h>

#define VOICE_RATE 11025
#define VOICE_MAX_SECONDS 60

/* Starts the (idle) capture thread. Returns < 0 on failure. */
int voice_init(void);
/* A new session: drops any recording in progress. */
void voice_reset(void);
void voice_start(void);
/* Release (cancel 0) sends what was recorded; cancel 1 discards it. */
void voice_stop(int cancel);
/* Main thread, once per frame: encodes and sends captured audio, finishes the utterance. */
void voice_poll(void);
int voice_active(void);
/* Peak level of the latest block, 0-100. */
int voice_level(void);
int voice_seconds(void);
/* VoiceTranscript, VoiceReplyText and Error notices; returns 1 if handled. */
int voice_on_message(const uint8_t *msg, size_t len);

#endif
