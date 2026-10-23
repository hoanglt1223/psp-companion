/* Triangle opens a 4x3 grid of the twelve host macro slots; only the slot number is sent. */
#ifndef PSPC_MACROS_H
#define PSPC_MACROS_H

#include <stdint.h>

#include "protocol.h"

void macropad_set_labels(const pspc_layout *layout);
void macropad_open(void);
void macropad_close(void);
int macropad_is_open(void);
/* D-pad or cursor selects, X sends, O or Triangle closes. */
void macropad_input(uint32_t pressed, int cursor_visible, int cx, int cy, uint64_t now_us);
void macropad_draw(uint64_t now_us);

#endif
