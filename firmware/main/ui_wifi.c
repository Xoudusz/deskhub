// Wi-Fi screen: the board's network and address, the token the app needs, the saved
// networks (Forget), and the networks nearby. Tapping one opens a password field with an
// on-screen keyboard. Opens from the Wi-Fi icon, and by itself when no network is saved.
#include <stdio.h>
#include <string.h>

#include "hubcfg.h"
#include "icons.h"
#include "net.h"
#include "ui.h"
#include "ui_internal.h"

#define ICON_LOCK_GLYPH "\xEE\xA2\x97"       /* e897 lock */
#define ICON_ARROW_BACK "\xEE\x97\x84"       /* e5c4 arrow_back */
#define ICON_VISIBILITY "\xEE\xA3\xB4"       /* e8f4 visibility */
#define ICON_VISIBILITY_OFF "\xEE\xA3\xB5"   /* e8f5 visibility_off */

#define WPAD 16
#define HEAD_H 64
#define LEFT_W 300
#define BODY_H (480 - HEAD_H - WPAD)
#define SAVED_MAX 5

static lv_obj_t *s_root, *s_close, *s_state, *s_addr, *s_token, *s_saved, *s_list, *s_scan_lbl;
static lv_obj_t *s_join, *s_join_title, *s_ta, *s_eye, *s_join_btn, *s_join_msg, *s_kb;
static lv_timer_t *s_timer;

static net_ap_t s_aps[NET_SCAN_MAX];   // the rows in the list, by index
static int s_nap;
static uint32_t s_gen = UINT32_MAX;
static char s_saved_names[SAVED_MAX][33];
static int s_nsaved;
static char s_cur[33];                 // the network shown as connected
static char s_ip_shown[16];
static bool s_want_scan;
static bool s_joining;
static net_ap_t s_join_ap;             // the network the join panel is for

static uint32_t signal_color(int rssi)
{
    return rssi >= -62 ? COL_GOOD : rssi >= -74 ? COL_WARN : COL_BAD;
}

static bool is_saved(const char *ssid)
{
    for (int i = 0; i < s_nsaved; i++) {
        if (strcmp(s_saved_names[i], ssid) == 0) return true;
    }
    return false;
}

static lv_obj_t *column(lv_obj_t *parent, int x, int w)
{
    lv_obj_t *c = ui_card(parent);
    lv_obj_set_pos(c, x, HEAD_H);
    lv_obj_set_size(c, w, BODY_H);
    lv_obj_set_style_pad_all(c, WPAD, 0);
    lv_obj_set_flex_flow(c, LV_FLEX_FLOW_COLUMN);
    lv_obj_set_style_pad_row(c, 6, 0);
    return c;
}

// ---- lists ----------------------------------------------------------------------

static void forget_cb(lv_event_t *e);
static void row_cb(lv_event_t *e);

static void rebuild_saved(void)
{
    s_nsaved = net_saved(s_saved_names, SAVED_MAX);
    lv_obj_clean(s_saved);
    if (s_nsaved == 0) {
        ui_label(s_saved, g_font_16, COL_MUTED, "None yet");
        return;
    }
    for (int i = 0; i < s_nsaved; i++) {
        lv_obj_t *row = lv_obj_create(s_saved);
        lv_obj_remove_style_all(row);
        lv_obj_set_size(row, LV_PCT(100), 40);
        lv_obj_set_flex_flow(row, LV_FLEX_FLOW_ROW);
        lv_obj_set_flex_align(row, LV_FLEX_ALIGN_START, LV_FLEX_ALIGN_CENTER, LV_FLEX_ALIGN_CENTER);
        lv_obj_remove_flag(row, LV_OBJ_FLAG_SCROLLABLE);
        lv_obj_t *name = ui_label(row, g_font_16, COL_TEXT, s_saved_names[i]);
        lv_obj_set_flex_grow(name, 1);
        lv_label_set_long_mode(name, LV_LABEL_LONG_DOT);
        lv_obj_t *b = ui_round_button(row, 32, COL_CARD_HI, g_icon_20, ICON_CLOSE, forget_cb, (void *)(intptr_t)i);
        lv_obj_set_ext_click_area(b, 6);
    }
}

static void rebuild_list(void)
{
    lv_obj_clean(s_list);
    for (int i = 0; i < s_nap; i++) {
        const net_ap_t *a = &s_aps[i];
        lv_obj_t *row = lv_obj_create(s_list);
        lv_obj_remove_style_all(row);
        lv_obj_set_size(row, LV_PCT(100), 52);
        lv_obj_set_style_radius(row, 12, 0);
        lv_obj_set_style_pad_hor(row, 12, 0);
        lv_obj_set_style_pad_column(row, 12, 0);
        lv_obj_set_style_bg_opa(row, LV_OPA_COVER, LV_STATE_PRESSED);
        lv_obj_set_style_bg_color(row, lv_color_hex(COL_PRESSED), LV_STATE_PRESSED);
        lv_obj_set_flex_flow(row, LV_FLEX_FLOW_ROW);
        lv_obj_set_flex_align(row, LV_FLEX_ALIGN_START, LV_FLEX_ALIGN_CENTER, LV_FLEX_ALIGN_CENTER);
        lv_obj_remove_flag(row, LV_OBJ_FLAG_SCROLLABLE);
        lv_obj_add_flag(row, LV_OBJ_FLAG_CLICKABLE);
        lv_obj_add_event_cb(row, row_cb, LV_EVENT_CLICKED, (void *)(intptr_t)i);

        ui_label(row, g_icon_24, signal_color(a->rssi), ICON_WIFI);
        lv_obj_t *name = ui_label(row, g_font_20, COL_TEXT, a->ssid);
        lv_obj_set_flex_grow(name, 1);
        lv_label_set_long_mode(name, LV_LABEL_LONG_DOT);
        if (strcmp(a->ssid, s_cur) == 0) {
            ui_label(row, g_font_14, COL_GOOD, "Connected");
        } else if (is_saved(a->ssid)) {
            ui_label(row, g_font_14, COL_ACCENT, "Saved");
        }
        if (a->secure) ui_label(row, g_icon_20, COL_DIM, ICON_LOCK_GLYPH);
    }
}

// ---- status ---------------------------------------------------------------------

static void refresh_status(bool force)
{
    char cur[33];
    net_ssid(cur, sizeof(cur));
    const char *ip = net_ip();
    if (!force && strcmp(cur, s_cur) == 0 && strcmp(ip, s_ip_shown) == 0) return;
    bool cur_changed = strcmp(cur, s_cur) != 0;
    strlcpy(s_cur, cur, sizeof(s_cur));
    strlcpy(s_ip_shown, ip, sizeof(s_ip_shown));

    char buf[96];
    if (cur[0]) {
        snprintf(buf, sizeof(buf), "Connected to %s", cur);
        lv_label_set_text(s_state, buf);
        const char *host = hubcfg()->host[0] ? hubcfg()->host : "deskhub";
        snprintf(buf, sizeof(buf), "%s  \xC2\xB7  %s.local", ip[0] ? ip : "...", host);
        lv_label_set_text(s_addr, buf);
    } else {
        lv_label_set_text(s_state, "Not connected");
        lv_label_set_text(s_addr, net_needs_setup() ? "Pick a network to get started" : "Looking for a saved network");
    }
    if (cur_changed) rebuild_list();   // the "Connected" tag moves
    // Closing is only possible once there is a network to go back to.
    if (net_needs_setup()) lv_obj_add_flag(s_close, LV_OBJ_FLAG_HIDDEN);
    else lv_obj_remove_flag(s_close, LV_OBJ_FLAG_HIDDEN);
}

static void refresh_scan(void)
{
    uint32_t gen;
    net_ap_t aps[NET_SCAN_MAX];
    int n = net_scan_results(aps, NET_SCAN_MAX, &gen);
    if (gen != s_gen) {
        s_gen = gen;
        memcpy(s_aps, aps, sizeof(aps));
        s_nap = n;
        rebuild_list();
    }
    char buf[40];
    if (s_want_scan || net_scanning()) snprintf(buf, sizeof(buf), "Scanning...");
    else if (s_nap == 0) snprintf(buf, sizeof(buf), "No networks found");
    else snprintf(buf, sizeof(buf), "%d network%s", s_nap, s_nap == 1 ? "" : "s");
    lv_label_set_text(s_scan_lbl, buf);
}

static void start_scan(void)
{
    s_want_scan = !net_scan_start();   // busy: the timer tries again
}

// ---- join panel -------------------------------------------------------------------

static void join_close(void)
{
    s_joining = false;
    lv_obj_add_flag(s_join, LV_OBJ_FLAG_HIDDEN);
    lv_textarea_set_text(s_ta, "");
}

static void join_set_busy(bool busy)
{
    if (busy) lv_obj_add_state(s_join_btn, LV_STATE_DISABLED);
    else lv_obj_remove_state(s_join_btn, LV_STATE_DISABLED);
}

static void join_go(void)
{
    if (s_joining) return;
    const char *pass = s_join_ap.secure ? lv_textarea_get_text(s_ta) : "";
    size_t len = strlen(pass);
    if (s_join_ap.secure && (len < 8 || len > 64)) {
        lv_label_set_text(s_join_msg, "Wi-Fi passwords have 8 to 64 characters.");
        lv_obj_set_style_text_color(s_join_msg, lv_color_hex(COL_WARN), 0);
        return;
    }
    net_join(s_join_ap.ssid, pass);
    s_joining = true;
    join_set_busy(true);
    lv_label_set_text(s_join_msg, "Connecting...");
    lv_obj_set_style_text_color(s_join_msg, lv_color_hex(COL_MUTED), 0);
}

static void join_open(const net_ap_t *ap, bool saved)
{
    s_join_ap = *ap;
    char buf[64];
    snprintf(buf, sizeof(buf), "Join %s", ap->ssid);
    lv_label_set_text(s_join_title, buf);
    lv_textarea_set_text(s_ta, "");
    lv_textarea_set_password_mode(s_ta, true);
    lv_label_set_text(lv_obj_get_child(s_eye, 0), ICON_VISIBILITY);
    join_set_busy(false);
    lv_obj_set_style_text_color(s_join_msg, lv_color_hex(COL_MUTED), 0);
    bool ask = ap->secure && !saved;
    if (ask) {
        lv_obj_remove_flag(s_ta, LV_OBJ_FLAG_HIDDEN);
        lv_obj_remove_flag(s_eye, LV_OBJ_FLAG_HIDDEN);
        lv_obj_remove_flag(s_kb, LV_OBJ_FLAG_HIDDEN);
        lv_label_set_text(s_join_msg, "Type the password, then Join.");
    } else {
        lv_obj_add_flag(s_ta, LV_OBJ_FLAG_HIDDEN);
        lv_obj_add_flag(s_eye, LV_OBJ_FLAG_HIDDEN);
        lv_obj_add_flag(s_kb, LV_OBJ_FLAG_HIDDEN);
        lv_label_set_text(s_join_msg, saved ? "Saved network." : "Open network: no password.");
    }
    lv_obj_remove_flag(s_join, LV_OBJ_FLAG_HIDDEN);
    if (saved) {   // nothing to type: join right away
        if (net_join_saved(ap->ssid)) {
            s_joining = true;
            join_set_busy(true);
            lv_label_set_text(s_join_msg, "Connecting...");
        }
    }
}

static void join_poll(void)
{
    if (!s_joining) return;
    net_join_t st = net_join_state();
    if (st == NET_JOIN_OK) {
        join_close();
        rebuild_saved();
        refresh_status(true);
        rebuild_list();
    } else if (st == NET_JOIN_FAIL) {
        s_joining = false;
        join_set_busy(false);
        lv_label_set_text(s_join_msg, s_join_ap.secure ? "Couldn't join. Check the password and try again."
                                                       : "Couldn't join this network.");
        lv_obj_set_style_text_color(s_join_msg, lv_color_hex(COL_BAD), 0);
    }
}

// ---- events -----------------------------------------------------------------------

static void row_cb(lv_event_t *e)
{
    int i = (int)(intptr_t)lv_event_get_user_data(e);
    if (i < 0 || i >= s_nap) return;
    if (strcmp(s_aps[i].ssid, s_cur) == 0) return;   // already on it
    join_open(&s_aps[i], is_saved(s_aps[i].ssid));
}

static void after_forget(void *unused)
{
    (void)unused;
    rebuild_saved();
    refresh_status(true);
    rebuild_list();
}

static void forget_cb(lv_event_t *e)
{
    int i = (int)(intptr_t)lv_event_get_user_data(e);
    if (i < 0 || i >= s_nsaved) return;
    char name[33];
    strlcpy(name, s_saved_names[i], sizeof(name));
    net_forget(name);
    // Later: the rebuild deletes the button this event is running on.
    lv_async_call(after_forget, NULL);
}

static void close_cb(lv_event_t *e)
{
    (void)e;
    join_close();
    lv_obj_add_flag(s_root, LV_OBJ_FLAG_HIDDEN);
    lv_timer_pause(s_timer);
}

static void rescan_cb(lv_event_t *e)
{
    (void)e;
    start_scan();
    refresh_scan();
}

static void back_cb(lv_event_t *e)
{
    (void)e;
    join_close();
}

static void join_cb(lv_event_t *e)
{
    (void)e;
    join_go();
}

static void eye_cb(lv_event_t *e)
{
    (void)e;
    bool hidden = lv_textarea_get_password_mode(s_ta);
    lv_textarea_set_password_mode(s_ta, !hidden);
    lv_label_set_text(lv_obj_get_child(s_eye, 0), hidden ? ICON_VISIBILITY_OFF : ICON_VISIBILITY);
}

static void kb_cb(lv_event_t *e)
{
    lv_event_code_t code = lv_event_get_code(e);
    if (code == LV_EVENT_READY) join_go();
    else if (code == LV_EVENT_CANCEL) join_close();
}

static void timer_cb(lv_timer_t *t)
{
    (void)t;
    if (s_want_scan) start_scan();
    // The saved list changes on the network task (a Forget, a join): follow it.
    char names[SAVED_MAX][33];
    int n = net_saved(names, SAVED_MAX);
    bool same = n == s_nsaved;
    for (int i = 0; same && i < n; i++) same = strcmp(names[i], s_saved_names[i]) == 0;
    if (!same) {
        rebuild_saved();
        rebuild_list();
    }
    refresh_status(false);
    refresh_scan();
    join_poll();
}

// ---- build ------------------------------------------------------------------------

static void build_join(void)
{
    s_join = lv_obj_create(s_root);
    lv_obj_remove_style_all(s_join);
    lv_obj_set_size(s_join, 800, 480);
    lv_obj_set_style_bg_color(s_join, lv_color_hex(COL_BG), 0);
    lv_obj_set_style_bg_opa(s_join, LV_OPA_COVER, 0);
    lv_obj_add_flag(s_join, LV_OBJ_FLAG_CLICKABLE);
    lv_obj_remove_flag(s_join, LV_OBJ_FLAG_SCROLLABLE);
    lv_obj_add_flag(s_join, LV_OBJ_FLAG_HIDDEN);

    lv_obj_t *back = ui_round_button(s_join, 44, COL_CARD, g_icon_24, ICON_ARROW_BACK, back_cb, NULL);
    lv_obj_set_pos(back, WPAD, 10);
    s_join_title = ui_label(s_join, g_font_24, COL_TEXT, "");
    lv_obj_set_pos(s_join_title, WPAD + 60, 18);
    lv_obj_set_width(s_join_title, 800 - 2 * WPAD - 60);
    lv_label_set_long_mode(s_join_title, LV_LABEL_LONG_DOT);

    s_ta = lv_textarea_create(s_join);
    lv_obj_set_pos(s_ta, WPAD, 68);
    lv_obj_set_size(s_ta, 560, 56);
    lv_textarea_set_one_line(s_ta, true);
    lv_textarea_set_password_mode(s_ta, true);
    lv_textarea_set_password_bullet(s_ta, "\xE2\x80\xA2");
    lv_textarea_set_max_length(s_ta, 64);
    lv_textarea_set_placeholder_text(s_ta, "Password");
    lv_obj_set_style_text_font(s_ta, g_font_20, 0);
    lv_obj_set_style_text_color(s_ta, lv_color_hex(COL_TEXT), 0);
    lv_obj_set_style_text_color(s_ta, lv_color_hex(COL_DIM), LV_PART_TEXTAREA_PLACEHOLDER);
    lv_obj_set_style_bg_color(s_ta, lv_color_hex(COL_CARD), 0);
    lv_obj_set_style_bg_opa(s_ta, LV_OPA_COVER, 0);
    lv_obj_set_style_border_color(s_ta, lv_color_hex(COL_ACCENT), 0);
    lv_obj_set_style_border_width(s_ta, 2, 0);
    lv_obj_set_style_radius(s_ta, 12, 0);
    lv_obj_set_style_pad_hor(s_ta, 16, 0);
    lv_obj_set_style_pad_ver(s_ta, 14, 0);
    lv_obj_set_style_bg_color(s_ta, lv_color_hex(COL_ACCENT), LV_PART_CURSOR);
    lv_obj_set_style_border_color(s_ta, lv_color_hex(COL_ACCENT), LV_PART_CURSOR);
    lv_obj_add_state(s_ta, LV_STATE_FOCUSED);   // shows the cursor

    s_eye = ui_round_button(s_join, 48, COL_CARD, g_icon_24, ICON_VISIBILITY, eye_cb, NULL);
    lv_obj_set_pos(s_eye, WPAD + 572, 72);

    s_join_btn = lv_button_create(s_join);
    lv_obj_remove_style_all(s_join_btn);
    lv_obj_set_size(s_join_btn, 132, 56);
    lv_obj_set_pos(s_join_btn, 800 - WPAD - 132, 68);
    lv_obj_set_style_radius(s_join_btn, 28, 0);
    lv_obj_set_style_bg_opa(s_join_btn, LV_OPA_COVER, 0);
    lv_obj_set_style_bg_color(s_join_btn, lv_color_hex(COL_ACCENT), 0);
    lv_obj_set_style_bg_color(s_join_btn, lv_color_hex(COL_ACCENT_PRESSED), LV_STATE_PRESSED);
    lv_obj_set_style_bg_color(s_join_btn, lv_color_hex(COL_CARD_HI), LV_STATE_DISABLED);
    lv_obj_t *jl = ui_label(s_join_btn, g_font_20, COL_ON_ACCENT, "Join");
    lv_obj_center(jl);
    lv_obj_add_event_cb(s_join_btn, join_cb, LV_EVENT_CLICKED, NULL);

    s_join_msg = ui_label(s_join, g_font_16, COL_MUTED, "");
    lv_obj_set_pos(s_join_msg, WPAD + 4, 136);
    lv_obj_set_width(s_join_msg, 800 - 2 * WPAD);

    s_kb = lv_keyboard_create(s_join);
    lv_obj_set_size(s_kb, 800, 300);
    lv_obj_align(s_kb, LV_ALIGN_BOTTOM_MID, 0, 0);
    lv_keyboard_set_textarea(s_kb, s_ta);
    // Montserrat: the keyboard's special keys are LVGL symbols, which only the built-in fonts have.
    lv_obj_set_style_text_font(s_kb, &lv_font_montserrat_20, 0);
    lv_obj_set_style_bg_color(s_kb, lv_color_hex(COL_RAIL), 0);
    lv_obj_set_style_bg_opa(s_kb, LV_OPA_COVER, 0);
    lv_obj_set_style_border_width(s_kb, 0, 0);
    lv_obj_set_style_pad_all(s_kb, 8, 0);
    lv_obj_set_style_pad_gap(s_kb, 6, 0);
    lv_obj_set_style_bg_color(s_kb, lv_color_hex(COL_CARD_HI), LV_PART_ITEMS);
    lv_obj_set_style_bg_opa(s_kb, LV_OPA_COVER, LV_PART_ITEMS);
    lv_obj_set_style_text_color(s_kb, lv_color_hex(COL_TEXT), LV_PART_ITEMS);
    lv_obj_set_style_border_width(s_kb, 0, LV_PART_ITEMS);
    lv_obj_set_style_shadow_width(s_kb, 0, LV_PART_ITEMS);
    lv_obj_set_style_radius(s_kb, 10, LV_PART_ITEMS);
    lv_obj_set_style_bg_color(s_kb, lv_color_hex(COL_PRESSED), LV_PART_ITEMS | LV_STATE_PRESSED);
    lv_obj_set_style_bg_color(s_kb, lv_color_hex(COL_CARD), LV_PART_ITEMS | LV_STATE_CHECKED);
    lv_obj_set_style_text_color(s_kb, lv_color_hex(COL_TEXT), LV_PART_ITEMS | LV_STATE_CHECKED);
    lv_obj_add_event_cb(s_kb, kb_cb, LV_EVENT_READY, NULL);
    lv_obj_add_event_cb(s_kb, kb_cb, LV_EVENT_CANCEL, NULL);
}

static void build(void)
{
    s_root = lv_obj_create(lv_layer_top());
    lv_obj_remove_style_all(s_root);
    lv_obj_set_size(s_root, 800, 480);
    lv_obj_set_style_bg_color(s_root, lv_color_hex(COL_BG), 0);
    lv_obj_set_style_bg_opa(s_root, LV_OPA_COVER, 0);
    lv_obj_add_flag(s_root, LV_OBJ_FLAG_CLICKABLE);   // nothing behind it reacts
    lv_obj_remove_flag(s_root, LV_OBJ_FLAG_SCROLLABLE);

    // Header: title, Rescan, Close.
    lv_obj_t *icon = ui_label(s_root, g_icon_32, COL_ACCENT, ICON_WIFI);
    lv_obj_set_pos(icon, WPAD, 14);
    lv_obj_t *title = ui_label(s_root, g_font_28, COL_TEXT, "Wi-Fi");
    lv_obj_set_pos(title, WPAD + 44, 14);
    s_close = ui_round_button(s_root, 44, COL_CARD, g_icon_24, ICON_CLOSE, close_cb, NULL);
    lv_obj_set_pos(s_close, 800 - WPAD - 44, 10);
    lv_obj_t *rescan = ui_round_button(s_root, 44, COL_CARD, g_icon_24, ICON_REFRESH, rescan_cb, NULL);
    lv_obj_set_pos(rescan, 800 - WPAD - 44 - 56, 10);

    // Left: this board.
    lv_obj_t *left = column(s_root, WPAD, LEFT_W);
    ui_caption(left, "THIS BOARD");
    s_state = ui_label(left, g_font_20, COL_TEXT, "");
    lv_obj_set_width(s_state, LV_PCT(100));
    lv_label_set_long_mode(s_state, LV_LABEL_LONG_DOT);
    s_addr = ui_label(left, g_font_14, COL_MUTED, "");
    lv_obj_set_width(s_addr, LV_PCT(100));
    lv_obj_t *cap = ui_caption(left, "APP TOKEN");
    lv_obj_set_style_margin_top(cap, 10, 0);
    s_token = ui_label(left, g_font_24, COL_ACCENT, "");
    lv_obj_set_width(s_token, LV_PCT(100));
    lv_obj_t *hint = ui_label(left, g_font_14, COL_MUTED, "Type it into the Desk Hub app: Connection, Board.");
    lv_obj_set_width(hint, LV_PCT(100));
    cap = ui_caption(left, "SAVED");
    lv_obj_set_style_margin_top(cap, 10, 0);
    s_saved = lv_obj_create(left);
    lv_obj_remove_style_all(s_saved);
    lv_obj_set_size(s_saved, LV_PCT(100), LV_SIZE_CONTENT);
    lv_obj_set_flex_flow(s_saved, LV_FLEX_FLOW_COLUMN);

    // Right: networks nearby.
    lv_obj_t *right = column(s_root, WPAD * 2 + LEFT_W, 800 - 3 * WPAD - LEFT_W);
    lv_obj_remove_flag(right, LV_OBJ_FLAG_SCROLLABLE);
    lv_obj_t *head = lv_obj_create(right);
    lv_obj_remove_style_all(head);
    lv_obj_set_size(head, LV_PCT(100), LV_SIZE_CONTENT);
    ui_caption(head, "NEARBY");
    s_scan_lbl = ui_label(head, g_font_14, COL_MUTED, "");
    lv_obj_align(s_scan_lbl, LV_ALIGN_RIGHT_MID, 0, 0);
    s_list = lv_obj_create(right);
    lv_obj_remove_style_all(s_list);
    lv_obj_set_width(s_list, LV_PCT(100));
    lv_obj_set_flex_grow(s_list, 1);
    lv_obj_set_flex_flow(s_list, LV_FLEX_FLOW_COLUMN);
    lv_obj_set_style_pad_row(s_list, 2, 0);
    lv_obj_set_scroll_dir(s_list, LV_DIR_VER);

    build_join();
    s_timer = lv_timer_create(timer_cb, 400, NULL);
    lv_timer_pause(s_timer);
}

void ui_wifi_show(void)
{
    if (!s_root) build();
    if (ui_saver_shown()) ui_saver_show(false);
    join_close();
    lv_label_set_text(s_token, hubcfg()->token[0] ? hubcfg()->token : "-");
    rebuild_saved();
    s_gen = UINT32_MAX;   // redraw the list
    refresh_status(true);
    start_scan();
    refresh_scan();
    lv_obj_remove_flag(s_root, LV_OBJ_FLAG_HIDDEN);
    lv_obj_move_foreground(s_root);
    lv_timer_resume(s_timer);
}

bool ui_wifi_shown(void)
{
    return s_root && !lv_obj_has_flag(s_root, LV_OBJ_FLAG_HIDDEN);
}
