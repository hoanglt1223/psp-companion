/* The widget screen: layout and values pushed by the host, three pages (L/R), cursor taps, macro pad. */
#ifndef PSPC_WIDGETS_H
#define PSPC_WIDGETS_H

#include <stddef.h>
#include <stdint.h>

#include "ui.h"

/* Forgets the layout and values; call when a new session starts. The asset cache is kept. */
void widgets_reset(void);
/* Returns 1 if the message was a widget message. */
int widgets_on_message(const uint8_t *msg, size_t len);
void widgets_input(uint32_t pressed, const ui_pad *pad, uint64_t now_us);
/* Draws the whole screen (call between ui_begin and ui_end). */
void widgets_draw(uint64_t now_us);

#endif
