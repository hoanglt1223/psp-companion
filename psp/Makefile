TARGET = companion
OBJS = main.o net.o crypto.o protocol.o rng.o storage.o ui.o wifi.o

CFLAGS = -O2 -G0 -Wall -Wextra -std=gnu99
CXXFLAGS = $(CFLAGS) -fno-exceptions -fno-rtti
ASFLAGS = $(CFLAGS)

BUILD_PRX = 1
PSP_FW_VERSION = 660
PSP_LARGE_MEMORY = 0

LIBS = -lpspgu -lpsputility -lpspnet_inet -lpspnet_apctl -lpspnet -lpsprtc -lpspaudio -lpspwlan

EXTRA_TARGETS = EBOOT.PBP
PSP_EBOOT_TITLE = PSP Companion

PSPSDK = $(shell psp-config --pspsdk-path)
include $(PSPSDK)/lib/build.mak
