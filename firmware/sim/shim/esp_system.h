#pragma once
#include "esp_err.h"
// The board restarts; the simulator throws out of the call and the app starts a new instance.
void esp_restart(void) __attribute__((noreturn));
