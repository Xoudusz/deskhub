#pragma once
#include "FreeRTOS.h"
// Single-threaded: nothing to wait for.
static inline void vTaskDelay(TickType_t t) { (void)t; }
