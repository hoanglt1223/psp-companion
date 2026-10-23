/* The assistant conversation: what the user said, Claude's streaming replies, and notices, word-wrapped
 * with the Vietnamese-capable font and scrollable with the D-pad. */
#ifndef PSPC_TEXT_VIEW_H
#define PSPC_TEXT_VIEW_H

#include <stdint.h>

enum tv_who { TV_YOU = 0, TV_CLAUDE = 1, TV_NOTICE = 2 };

void tv_reset(void);
/* Adds a new entry. */
void tv_add(int who, uint32_t utterance_id, const char *text);
/* Appends streamed reply text to the reply for this utterance (creating it on the first delta). */
void tv_append_reply(uint32_t utterance_id, const char *delta);
void tv_set_status(const char *status, uint32_t color);
/* Positive scrolls back in history. */
void tv_scroll(int lines);
/* Draws the full screen; level is 0-100 while recording, -1 otherwise. */
void tv_draw(int level, int recording_seconds);

#endif
