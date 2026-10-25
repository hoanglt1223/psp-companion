/* The START menu: Screen, Assistant, Macros, Stream Deck, PC speaker, and Settings. */
#ifndef PSPC_MENU_H
#define PSPC_MENU_H

#include <stdint.h>

typedef enum {
    MENU_NONE,
    MENU_CLOSE,
    MENU_SCREEN,
    MENU_ASSISTANT,
    MENU_MACROS,
    MENU_DECK,
    MENU_SPEAKER,
    MENU_STATUS,
    MENU_WIFI,
    MENU_FORGET
} menu_action;

void menu_open(void);
void menu_close(void);
int menu_is_open(void);
/* Handles the buttons pressed this frame; the menu closes itself on any action but MENU_NONE. */
menu_action menu_input(uint32_t pressed);
/* Draws rows 4-28 (between ui_begin and ui_end, under the caller's header). */
void menu_draw(void);
/* Button hints for the footer. */
const char *menu_keys(void);

#endif
