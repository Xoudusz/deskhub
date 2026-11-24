// Simulator entry: LVGL on an 800x480 RGB565 display flushed into an RGBA buffer that the app
// draws on a canvas, a pointer from the mouse, and the exports the app calls.
#include <stdint.h>
#include <string.h>

#include <emscripten.h>

#include "hub_dispatch.h"
#include "hub_model.h"
#include "hubcfg.h"
#include "lvgl.h"
#include "ui.h"
#include "ui_internal.h"
#include "weather.h"

#define W 800
#define H 480
#define DRAW_LINES 48

static uint8_t s_rgba[W * H * 4];
static uint16_t s_draw[W * DRAW_LINES];
static int s_dirty;
static int32_t s_px, s_py;
static bool s_down;

static uint32_t tick_ms(void) { return (uint32_t)emscripten_get_now(); }

static void flush_cb(lv_display_t *disp, const lv_area_t *a, uint8_t *px)
{
    const uint16_t *src = (const uint16_t *)px;
    int32_t w = lv_area_get_width(a);
    for (int32_t y = a->y1; y <= a->y2; y++) {
        uint8_t *dst = s_rgba + ((size_t)y * W + a->x1) * 4;
        for (int32_t x = 0; x < w; x++) {
            uint16_t p = *src++;
            uint8_t r = (p >> 11) & 0x1f, g = (p >> 5) & 0x3f, b = p & 0x1f;
            dst[0] = (uint8_t)((r << 3) | (r >> 2));
            dst[1] = (uint8_t)((g << 2) | (g >> 4));
            dst[2] = (uint8_t)((b << 3) | (b >> 2));
            dst[3] = 255;
            dst += 4;
        }
    }
    s_dirty = 1;
    lv_display_flush_ready(disp);
}

static void pointer_cb(lv_indev_t *indev, lv_indev_data_t *data)
{
    (void)indev;
    data->point.x = s_px;
    data->point.y = s_py;
    data->state = s_down ? LV_INDEV_STATE_PRESSED : LV_INDEV_STATE_RELEASED;
}

EMSCRIPTEN_KEEPALIVE void sim_init(void)
{
    lv_init();
    lv_tick_set_cb(tick_ms);
    lv_display_t *disp = lv_display_create(W, H);
    lv_display_set_color_format(disp, LV_COLOR_FORMAT_RGB565);
    lv_display_set_buffers(disp, s_draw, NULL, sizeof(s_draw), LV_DISPLAY_RENDER_MODE_PARTIAL);
    lv_display_set_flush_cb(disp, flush_cb);
    lv_indev_t *in = lv_indev_create();
    lv_indev_set_type(in, LV_INDEV_TYPE_POINTER);
    lv_indev_set_read_cb(in, pointer_cb);
    hubcfg_load();
    ui_init();
}

// One step: runs LVGL timers and redraws. Returns ms until the next step is due.
EMSCRIPTEN_KEEPALIVE uint32_t sim_step(void) { return lv_timer_handler(); }

EMSCRIPTEN_KEEPALIVE uint8_t *sim_pixels(void) { return s_rgba; }

// 1 when the picture changed since the last call.
EMSCRIPTEN_KEEPALIVE int sim_take_dirty(void)
{
    int d = s_dirty;
    s_dirty = 0;
    return d;
}

EMSCRIPTEN_KEEPALIVE void sim_pointer(int x, int y, int down)
{
    s_px = x < 0 ? 0 : x >= W ? W - 1 : x;
    s_py = y < 0 ? 0 : y >= H ? H - 1 : y;
    s_down = down != 0;
}

// Shows the Home page (the app's Home builder previews it).
EMSCRIPTEN_KEEPALIVE void sim_show_home(void) { ui_show_page(PAGE_HOME); }

// A JSON message, as the board gets it over the WebSocket (no hello needed).
EMSCRIPTEN_KEEPALIVE void sim_json(const char *text) { hub_dispatch_text(text); }

// An ART1/STM1 frame. Returns 1 when it was valid.
EMSCRIPTEN_KEEPALIVE int sim_binary(const uint8_t *buf, int len) { return hub_dispatch_binary(buf, (size_t)len); }

// The Open-Meteo URL for the current location; the app fetches it and passes the body back.
EMSCRIPTEN_KEEPALIVE const char *sim_weather_url(void)
{
    static char url[512];
    weather_build_url(url, sizeof(url));
    return url;
}

EMSCRIPTEN_KEEPALIVE int sim_weather(const char *body)
{
    static hub_weather_t w;
    if (!weather_parse(body, &w)) return 0;
    ui_set_weather(&w);
    return 1;
}
