#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "hub_model.h"

// Fetches Open-Meteo every 15 minutes (on its own task) and pushes it to the UI.
void weather_start(void);

// The Open-Meteo forecast URL for the board's location (hubcfg) and units.
void weather_build_url(char *out, size_t n);
// Parses an Open-Meteo response into `w`. False when it lacks current/hourly/daily.
bool weather_parse(const char *body, hub_weather_t *w);

// Material icon glyph and a short description for a WMO weather code.
const char *weather_icon(int code, bool is_day);
const char *weather_text(int code);
// Tint for the weather icon: amber sun, violet night, cyan rain, and so on.
uint32_t weather_color(int code, bool is_day);
