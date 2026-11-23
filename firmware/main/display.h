#pragma once

#include <stdbool.h>
#include <stdint.h>
#include "esp_err.h"

// Starts LVGL, the display driver, touch input and the LVGL task.
esp_err_t display_start(void);

// Every LVGL call from outside the LVGL task must hold this lock.
void display_lock(void);
void display_unlock(void);

// Screen power. A touch while the screen is off only wakes it.
void display_wake(void);

// Follow the PC's display: while it is asleep the board goes dark (backlight off) or
// shows the clock screensaver. A touch brings the board back for a minute.
enum { PC_SLEEP_NONE, PC_SLEEP_OFF, PC_SLEEP_CLOCK };
void display_set_pc_sleep_mode(int mode);
void display_set_pc_asleep(bool asleep);
// Night mode: dark (mode PC_SLEEP_OFF or PC_SLEEP_CLOCK) every day from start_min to
// end_min (minutes after midnight, may cross midnight); start_min -1 turns it off.
// Saved to NVS so it works without the PC.
void display_set_night(int start_min, int end_min, int mode);
// Backlight level: day_pct (1-100) normally, night_pct during night mode.
// night_pct 0 keeps the night style (dark or clock); above 0 the board stays on,
// dimmed, and a "clock" night style shows the clock dimmed. Kept on the board.
void display_set_brightness(int day_pct, int night_pct);

// Simulate a short tap at (x, y), for remote testing.
void display_inject_tap(uint16_t x, uint16_t y);
// Simulate a horizontal drag from x to x2 at height y, lasting ms.
void display_inject_drag(uint16_t x, uint16_t y, uint16_t x2, int ms);

// Rendering performance over the last second (frames that actually redrew something).
typedef struct {
    uint32_t fps;       // frames rendered in the last second
    float avg_ms;       // average render+flush time per frame
    float max_ms;
    uint32_t px_per_s;  // pixels redrawn in the last second
} display_perf_t;
display_perf_t display_perf(void);
// Force a full-screen redraw now and return how long it took, in microseconds.
// Call with display_lock() held.
int64_t display_time_full_redraw(void);
