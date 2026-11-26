// Timer page: presets, a countdown ring, start/pause and reset. It runs on the
// board alone, so it works without the PC. While it runs, the rail tab shows the
// time left; when it ends, a full-screen card asks for a tap.
#include <stdio.h>
#include <string.h>

#include "audio.h"
#include "display.h"
#include "esp_timer.h"
#include "icons.h"
#include "ui_internal.h"

#define RING 300
static const int PRESETS[] = {1, 5, 10, 15, 25, 45};
#define N_PRESETS ((int)(sizeof(PRESETS) / sizeof(PRESETS[0])))

typedef enum { T_IDLE, T_RUNNING, T_PAUSED } tstate_t;

static tstate_t s_state = T_IDLE;
static int s_total = 25 * 60;   // seconds the timer was set to
static int64_t s_end_us;        // when running: esp_timer time it ends
static int s_left = 25 * 60;    // when idle or paused: seconds left

static lv_obj_t *s_arc, *s_time, *s_status, *s_go, *s_go_lbl, *s_preset[N_PRESETS];
static lv_obj_t *s_done, *s_done_sub;
static lv_timer_t *s_tick;

static int seconds_left(void)
{
    if (s_state != T_RUNNING) return s_left;
    int64_t us = s_end_us - esp_timer_get_time();
    return us <= 0 ? 0 : (int)((us + 999999) / 1000000);
}

static void fmt_mmss(char *buf, size_t n, int s)
{
    if (s >= 3600) snprintf(buf, n, "%d:%02d:%02d", s / 3600, (s / 60) % 60, s % 60);
    else snprintf(buf, n, "%d:%02d", s / 60, s % 60);
}

static void refresh(void)
{
    int left = seconds_left();
    char buf[16];
    fmt_mmss(buf, sizeof(buf), left);
    lv_label_set_text(s_time, buf);
    // The ring empties as time runs out (1000 steps so it moves smoothly).
    lv_arc_set_value(s_arc, s_total > 0 ? (int)(1000LL * left / s_total) : 0);

    static const char *status[] = {"Ready", "Running", "Paused"};
    lv_label_set_text(s_status, status[s_state]);
    lv_label_set_text(s_go_lbl, s_state == T_RUNNING ? ICON_PAUSE : ICON_PLAY_ARROW);
    for (int i = 0; i < N_PRESETS; i++) {
        bool sel = PRESETS[i] * 60 == s_total;
        lv_obj_set_style_border_color(s_preset[i], lv_color_hex(sel ? COL_ACCENT : COL_BORDER), 0);
        lv_obj_set_style_text_color(lv_obj_get_child(s_preset[i], 0), lv_color_hex(sel ? COL_TEXT : COL_MUTED), 0);
    }

    // Countdown on the rail tab while the timer is set, the icon otherwise.
    if (s_state == T_IDLE) ui_rail_set_text(PAGE_TIMER, NULL);
    else ui_rail_set_text(PAGE_TIMER, buf);
}

static void show_done(void)
{
    char buf[48];
    if (s_total % 60 == 0) snprintf(buf, sizeof(buf), "%d-minute timer", s_total / 60);
    else snprintf(buf, sizeof(buf), "%d-second timer", s_total);
    lv_label_set_text(s_done_sub, buf);
    lv_obj_remove_flag(s_done, LV_OBJ_FLAG_HIDDEN);
    lv_obj_move_foreground(s_done);
    display_wake();
}

static void tick_cb(lv_timer_t *t)
{
    (void)t;
    static int s_last = -1;
    if (s_state != T_RUNNING) return;
    int left = seconds_left();
    if (left == s_last) return;   // redraw once per second, not every tick
    s_last = left;
    if (left <= 0) {
        s_state = T_IDLE;
        s_left = s_total;
        lv_timer_pause(s_tick);
        refresh();
        show_done();
    sounds_event(SOUND_EV_TIMER);
        return;
    }
    refresh();
}

static void start(void)
{
    s_end_us = esp_timer_get_time() + (int64_t)s_left * 1000000;
    s_state = T_RUNNING;
    lv_timer_resume(s_tick);
}

static void go_cb(lv_event_t *e)
{
    (void)e;
    if (s_state == T_RUNNING) {
        s_left = seconds_left();
        s_state = T_PAUSED;
        lv_timer_pause(s_tick);
    } else {
        start();
    }
    refresh();
}

static void reset_cb(lv_event_t *e)
{
    (void)e;
    s_state = T_IDLE;
    s_left = s_total;
    lv_timer_pause(s_tick);
    refresh();
}

static void preset_cb(lv_event_t *e)
{
    int min = (int)(intptr_t)lv_event_get_user_data(e);
    s_total = s_left = min * 60;
    bool was_running = s_state == T_RUNNING;
    s_state = T_IDLE;
    if (was_running) start();   // switching preset while running restarts it
    refresh();
}

static void done_dismiss_cb(lv_event_t *e)
{
    (void)e;
    lv_obj_add_flag(s_done, LV_OBJ_FLAG_HIDDEN);
}

static void done_again_cb(lv_event_t *e)
{
    (void)e;
    lv_obj_add_flag(s_done, LV_OBJ_FLAG_HIDDEN);
    s_left = s_total;
    start();
    refresh();
}

static lv_obj_t *pill(lv_obj_t *parent, int w, int h, uint32_t bg, const lv_font_t *font, const char *text,
                      lv_event_cb_t cb, void *user)
{
    lv_obj_t *b = lv_button_create(parent);
    lv_obj_remove_style_all(b);
    lv_obj_set_size(b, w, h);
    lv_obj_set_style_radius(b, h / 2, 0);
    lv_obj_set_style_bg_color(b, lv_color_hex(bg), 0);
    lv_obj_set_style_bg_opa(b, LV_OPA_COVER, 0);
    lv_obj_set_style_border_color(b, lv_color_hex(COL_BORDER), 0);
    lv_obj_set_style_border_width(b, bg == COL_ACCENT ? 0 : 1, 0);
    lv_obj_set_style_bg_color(b, lv_color_hex(COL_ACCENT_PRESSED), LV_STATE_PRESSED);
    lv_obj_set_ext_click_area(b, 6);
    lv_obj_t *l = ui_label(b, font, bg == COL_ACCENT ? COL_BG : COL_TEXT, text);
    lv_obj_center(l);
    lv_obj_add_event_cb(b, cb, LV_EVENT_CLICKED, user);
    return b;
}

static void build_done(void)
{
    s_done = lv_obj_create(lv_layer_top());
    lv_obj_remove_style_all(s_done);
    lv_obj_set_size(s_done, 800, 480);
    lv_obj_set_style_bg_color(s_done, lv_color_hex(COL_BG), 0);
    lv_obj_set_style_bg_opa(s_done, LV_OPA_COVER, 0);
    lv_obj_add_flag(s_done, LV_OBJ_FLAG_CLICKABLE | LV_OBJ_FLAG_HIDDEN);
    lv_obj_remove_flag(s_done, LV_OBJ_FLAG_SCROLLABLE);

    lv_obj_t *card = ui_card(s_done);
    lv_obj_set_size(card, 560, 360);
    lv_obj_center(card);
    lv_obj_set_style_border_color(card, lv_color_hex(COL_GOOD), 0);
    lv_obj_set_style_border_width(card, 2, 0);
    lv_obj_set_style_pad_all(card, 32, 0);

    lv_obj_t *icon = ui_label(card, g_icon_80, COL_GOOD, ICON_TIMER);
    lv_obj_align(icon, LV_ALIGN_TOP_MID, 0, 0);
    lv_obj_t *t = ui_label(card, g_font_48, COL_TEXT, "Time's up");
    lv_obj_align(t, LV_ALIGN_TOP_MID, 0, 96);
    s_done_sub = ui_label(card, g_font_20, COL_MUTED, "");
    lv_obj_align(s_done_sub, LV_ALIGN_TOP_MID, 0, 156);

    lv_obj_t *again = pill(card, 220, 64, COL_CARD_HI, g_font_24, "Again", done_again_cb, NULL);
    lv_obj_align(again, LV_ALIGN_BOTTOM_LEFT, 0, 0);
    lv_obj_t *ok = pill(card, 220, 64, COL_ACCENT, g_font_24, "Done", done_dismiss_cb, NULL);
    lv_obj_align(ok, LV_ALIGN_BOTTOM_RIGHT, 0, 0);
}

void timer_page_create(lv_obj_t *page)
{
    // Left: the ring with the time in it.
    lv_obj_t *left = ui_card(page);
    lv_obj_set_size(left, 400, CONTENT_H);
    lv_obj_set_pos(left, 0, 0);

    s_arc = lv_arc_create(left);
    lv_obj_set_size(s_arc, RING, RING);
    lv_obj_align(s_arc, LV_ALIGN_CENTER, 0, -10);
    lv_arc_set_rotation(s_arc, 270);
    lv_arc_set_bg_angles(s_arc, 0, 360);
    lv_arc_set_range(s_arc, 0, 1000);
    lv_obj_remove_style(s_arc, NULL, LV_PART_KNOB);
    lv_obj_remove_flag(s_arc, LV_OBJ_FLAG_CLICKABLE);
    lv_obj_set_style_arc_width(s_arc, 16, LV_PART_MAIN);
    lv_obj_set_style_arc_width(s_arc, 16, LV_PART_INDICATOR);
    lv_obj_set_style_arc_rounded(s_arc, true, LV_PART_INDICATOR);
    lv_obj_set_style_arc_color(s_arc, lv_color_hex(COL_CARD_HI), LV_PART_MAIN);
    lv_obj_set_style_arc_color(s_arc, lv_color_hex(COL_ACCENT), LV_PART_INDICATOR);

    s_time = ui_label(left, ui_timer_font(), COL_TEXT, "");
    lv_obj_align_to(s_time, s_arc, LV_ALIGN_CENTER, 0, -10);
    s_status = ui_label(left, g_font_20, COL_MUTED, "");
    lv_obj_align_to(s_status, s_arc, LV_ALIGN_CENTER, 0, 50);
    lv_obj_set_style_text_align(s_time, LV_TEXT_ALIGN_CENTER, 0);
    lv_obj_set_width(s_time, RING - 40);
    lv_obj_set_style_text_align(s_status, LV_TEXT_ALIGN_CENTER, 0);
    lv_obj_set_width(s_status, RING - 40);
    lv_obj_align_to(s_time, s_arc, LV_ALIGN_CENTER, 0, -10);
    lv_obj_align_to(s_status, s_arc, LV_ALIGN_CENTER, 0, 50);

    // Right: presets, then start/pause and reset.
    const int rx = 412, rw = CONTENT_W - rx;
    lv_obj_t *right = ui_card(page);
    lv_obj_set_size(right, rw, CONTENT_H);
    lv_obj_set_pos(right, rx, 0);
    lv_obj_t *cap = ui_caption(right, "MINUTES");
    lv_obj_set_pos(cap, 0, 0);
    const int bw = (rw - 32 - 12) / 2, bh = 60;
    for (int i = 0; i < N_PRESETS; i++) {
        char buf[8];
        snprintf(buf, sizeof(buf), "%d", PRESETS[i]);
        s_preset[i] = pill(right, bw, bh, COL_CARD_HI, g_font_24, buf, preset_cb,
                           (void *)(intptr_t)PRESETS[i]);
        lv_obj_set_style_radius(s_preset[i], 16, 0);
        lv_obj_set_pos(s_preset[i], (i % 2) * (bw + 12), 28 + (i / 2) * (bh + 12));
    }

    lv_obj_t *reset = ui_round_button(right, 72, COL_CARD, g_icon_32, ICON_REPLAY, reset_cb, NULL);
    lv_obj_align(reset, LV_ALIGN_BOTTOM_LEFT, 12, 0);
    s_go = ui_round_button(right, 88, COL_ACCENT, g_icon_48, ICON_PLAY_ARROW, go_cb, NULL);
    lv_obj_align(s_go, LV_ALIGN_BOTTOM_RIGHT, -12, 0);
    s_go_lbl = lv_obj_get_child(s_go, 0);
    lv_obj_set_style_text_color(s_go_lbl, lv_color_hex(COL_BG), 0);

    build_done();
    s_tick = lv_timer_create(tick_cb, 250, NULL);
    lv_timer_pause(s_tick);
    refresh();
}
