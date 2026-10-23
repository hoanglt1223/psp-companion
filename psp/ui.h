#ifndef PSPC_UI_H
#define PSPC_UI_H

#include <stddef.h>
#include <stdint.h>

#define UI_COLS 60
#define UI_ROWS 34

/* Colours are 0xAABBGGRR. */
#define UI_WHITE 0xFFFFFFFF
#define UI_GREY 0xFFA0A0A0
#define UI_GREEN 0xFF60E060
#define UI_RED 0xFF6060FF
#define UI_YELLOW 0xFF40E0F0
#define UI_CYAN 0xFFF0C040

void ui_init(void);
void ui_begin(void);
void ui_end(void);
/* Prints at a character cell (8x8 px). */
void ui_text(int col, int row, uint32_t color, const char *fmt, ...) __attribute__((format(printf, 4, 5)));

/* Pumps one frame while a system utility dialog is visible. */
void ui_dialog_frame_begin(void);
void ui_dialog_frame_end(void);

#define UI_STRIDE 512
#define UI_WIDTH 480
#define UI_HEIGHT 272

typedef struct {
    uint32_t buttons; /* held */
    uint8_t lx, ly;   /* analog nub, 128 is centre */
} ui_pad;

/* The pad as sampled by the last ui_poll_pressed(). */
const ui_pad *ui_pad_state(void);

/* Uncached pointer to the buffer being drawn, UI_STRIDE pixels per row (0xAABBGGRR).
 * Valid between ui_begin() and ui_end(). */
uint32_t *ui_framebuffer(void);

/* Buttons newly pressed since the previous call. */
uint32_t ui_poll_pressed(void);

/* Shows the on-screen keyboard. Returns 0 when the user confirmed text, -1 when cancelled. */
int ui_osk(const char *title, const char *initial, int max_len, char *out, size_t out_cap);

#endif
