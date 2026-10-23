/* Analog-nub cursor with acceleration; it hides after a few seconds without movement. */
#ifndef PSPC_CURSOR_H
#define PSPC_CURSOR_H

#include <stdint.h>

#include "ui.h"

void cursor_reset(void);
void cursor_update(const ui_pad *pad, uint64_t now_us);
int cursor_visible(void);
int cursor_x(void);
int cursor_y(void);
void cursor_draw(void);

#endif
