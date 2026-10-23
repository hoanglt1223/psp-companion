/* Stream Deck mode: a grid of live Bitfocus Companion buttons drawn from host-sent key images. */
#ifndef PSPC_STREAMDECK_H
#define PSPC_STREAMDECK_H

#include <stddef.h>
#include <stdint.h>

#include "ui.h"

/* A new session: forgets the layout and key assignments (decoded images stay cached). */
void deck_reset(void);
/* DeckLayout and DeckKeyImage; returns 1 if handled. */
int deck_on_message(const uint8_t *msg, size_t len);
/* D-pad or cursor selects, X presses (release is sent separately), L/R switch Companion pages,
 * Triangle opens the macro pad. */
void deck_input(uint32_t pressed, const ui_pad *pad, uint64_t now_us);
void deck_draw(uint64_t now_us);
/* Called when the screen is left, so a held key is released on the PC. */
void deck_release(void);

#endif
