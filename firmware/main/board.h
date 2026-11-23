// Board layer. Two boards, picked by the build target:
//  - esp32s3: Waveshare ESP32-S3-Touch-LCD-4.3B (board.c): 800x480 RGB panel,
//    GT911 touch, CH422G I/O expander for touch reset, LCD reset and backlight.
//  - esp32p4: Waveshare ESP32-P4-WIFI6-Touch-LCD-4.3 (board_p4.c): 480x800
//    MIPI-DSI panel turned to landscape, GT911 touch, PWM backlight.
#pragma once

#include <stdbool.h>
#include <stdint.h>
#include "esp_err.h"
#include "esp_lcd_types.h"

#include "sdkconfig.h"

// The UI is always 800x480 landscape.
#define BOARD_WIDTH  800
#define BOARD_HEIGHT 480

#if CONFIG_IDF_TARGET_ESP32P4
// The panel itself is portrait; display.c rotates the UI onto it.
#define BOARD_PANEL_W   480
#define BOARD_PANEL_H   800
#define BOARD_ROTATED   1

// Where UI point (lx, ly) lands on the panel. Matches LVGL's rotation of the
// same direction (display.c), including how LVGL rotates touch points.
static inline void board_logical_to_panel(int lx, int ly, uint16_t *nx, uint16_t *ny)
{
#if CONFIG_HUB_ROTATION_270
    *nx = BOARD_PANEL_W - 1 - ly;
    *ny = lx;
#else
    *nx = ly;
    *ny = BOARD_PANEL_H - 1 - lx;
#endif
}
#else
#define BOARD_PANEL_W   BOARD_WIDTH
#define BOARD_PANEL_H   BOARD_HEIGHT
#define BOARD_ROTATED   0
#endif

esp_err_t board_init(void);
esp_lcd_panel_handle_t board_panel(void);
void board_backlight(bool on);
// Backlight level 1-100 % while on. Boards without dimming ignore it.
void board_backlight_set(uint8_t pct);
bool board_has_dimming(void);
// Returns true while a finger is down; x/y get the first touch point in panel
// coordinates (BOARD_PANEL_W x BOARD_PANEL_H).
bool board_touch_read(uint16_t *x, uint16_t *y);
// Copies the current frame as 800x480 RGB565 in UI orientation. The caller
// holds the display lock.
bool board_snapshot(uint16_t *dst);
