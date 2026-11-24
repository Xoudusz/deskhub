#pragma once
// The embedded TTF fonts as start/end pointers. On the board they come from EMBED_FILES
// (main/CMakeLists.txt); the simulator (firmware/sim) links them as C arrays.
#include <stdint.h>

#ifdef HUB_SIM
extern const uint8_t sim_icons_ttf[], sim_text_ttf[], sim_clock_ttf[];
extern const unsigned sim_icons_ttf_len, sim_text_ttf_len, sim_clock_ttf_len;
#define icons_ttf_start sim_icons_ttf
#define icons_ttf_end (sim_icons_ttf + sim_icons_ttf_len)
#define text_ttf_start sim_text_ttf
#define text_ttf_end (sim_text_ttf + sim_text_ttf_len)
#define clock_ttf_start sim_clock_ttf
#define clock_ttf_end (sim_clock_ttf + sim_clock_ttf_len)
#else
extern const uint8_t icons_ttf_start[] asm("_binary_MaterialIcons_Regular_ttf_start");
extern const uint8_t icons_ttf_end[] asm("_binary_MaterialIcons_Regular_ttf_end");
// Inter: Medium for text, SemiBold for large numbers and the clock.
extern const uint8_t text_ttf_start[] asm("_binary_Inter_Medium_ttf_start");
extern const uint8_t text_ttf_end[] asm("_binary_Inter_Medium_ttf_end");
extern const uint8_t clock_ttf_start[] asm("_binary_Inter_SemiBold_ttf_start");
extern const uint8_t clock_ttf_end[] asm("_binary_Inter_SemiBold_ttf_end");
#endif
