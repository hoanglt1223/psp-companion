TARGET = companion
OBJS = main.o net.o crypto.o protocol.o rng.o storage.o ui.o wifi.o \
       font.o font_vi.o image.o gfx.o cursor.o macros.o widgets.o \
       jitter.o mp3_player.o audio_stream.o \
       adpcm.o voice.o text_view.o streamdeck.o \
       power.o menu.o

CFLAGS = -O2 -G0 -Wall -Wextra -std=gnu99
CXXFLAGS = $(CFLAGS) -fno-exceptions -fno-rtti
ASFLAGS = $(CFLAGS)

BUILD_PRX = 1
PSP_FW_VERSION = 660
PSP_LARGE_MEMORY = 0

LIBS = -lpspgu -lpspge -lpspaudiocodec -lpsputility -lpspnet_inet -lpspnet_apctl -lpspnet -lpsprtc -lpspaudio -lpspwlan -lpsppower

EXTRA_TARGETS = EBOOT.PBP
PSP_EBOOT_TITLE = PSP Companion
PSP_EBOOT_ICON = ICON0.PNG
PSP_EBOOT_PIC1 = PIC1.PNG

PSPSDK = $(shell psp-config --pspsdk-path)
include $(PSPSDK)/lib/build.mak

# The save utility shows ICON0 next to the save data, so it is embedded as well.
icon0_png.h: ICON0.PNG
	bin2c $< $@ icon0_png

storage.o: icon0_png.h

EXTRA_CLEAN = icon0_png.h
