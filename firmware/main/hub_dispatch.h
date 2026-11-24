#pragma once
// Board messages -> UI, shared by hub_server.c and the simulator (firmware/sim).
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "cJSON.h"

// One JSON message (already parsed; `type` is its "type"). Takes display_lock() itself.
void hub_dispatch_json(const cJSON *m, const char *type);

// Parses and dispatches one JSON text frame (the simulator's entry; no hello/auth).
void hub_dispatch_text(const char *text);

// ART1 album art or STM1 Steam art. Returns true when the frame was valid and shown.
bool hub_dispatch_binary(const uint8_t *buf, size_t len);
