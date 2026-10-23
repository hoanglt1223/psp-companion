/* MP3 frame decoding on the Media Engine (sceAudiocodec). */
#ifndef PSPC_MP3_PLAYER_H
#define PSPC_MP3_PLAYER_H

#include <stddef.h>
#include <stdint.h>

#define MP3P_SAMPLES 1152

/* Loads the AV codec module and sets up the decoder. Returns 0, or < 0 (a PSP error code). */
int mp3p_init(void);
/* Decodes one MPEG-1 Layer III frame to MP3P_SAMPLES stereo 16-bit samples (mono frames come out as
 * stereo). pcm must be 64-byte aligned. Returns 0, or < 0 when the frame did not decode. */
int mp3p_decode(const uint8_t *frame, size_t len, int16_t *pcm);
void mp3p_term(void);

#endif
