// Internet page: latency with jitter and loss, online status and the last
// outage, data used today and this month, a latency chart, and the last speed
// test with a button to run one (the companion does the measuring).
#include <stdio.h>
#include <string.h>

#include "hub_server.h"
#include "icons.h"
#include "ui.h"
#include "ui_internal.h"

#define CARD_W ((CONTENT_W - 2 * 12) / 3)
// Sizes for the Internet view inside the Stats page (388 px tall).
#define VIEW_H (CONTENT_H - 52)            // 388 beside a rail, 320 above the dock
#define TOP_H (UI_COMPACT ? 116 : 130)
#define BOT_H (UI_COMPACT ? 96 : 124)
#define CHART_H (VIEW_H - TOP_H - BOT_H - 24)   // 110 beside a rail

hub_net_t g_net;

static lv_obj_t *s_lat, *s_lat_sub, *s_status, *s_drop, *s_today, *s_month;
static lv_obj_t *s_chart, *s_chart_max, *s_chart_cap, *s_offline;
static lv_chart_series_t *s_ser;
static lv_obj_t *s_down, *s_upl, *s_sp_sub, *s_now, *s_run, *s_run_lbl;

void ui_fmt_bytes(char *buf, size_t n, double b)
{
    if (b >= 1e12) snprintf(buf, n, "%.2f TB", b / 1e12);
    else if (b >= 1e9) snprintf(buf, n, "%.1f GB", b / 1e9);
    else if (b >= 1e6) snprintf(buf, n, "%.0f MB", b / 1e6);
    else snprintf(buf, n, "%.0f KB", b / 1e3);
}

void ui_fmt_ago(char *buf, size_t n, time_t t)
{
    long d = (long)(time(NULL) - t);
    if (t <= 0) snprintf(buf, n, "never");
    else if (d < 60) snprintf(buf, n, "just now");
    else if (d < 3600) snprintf(buf, n, "%ldm ago", d / 60);
    else if (d < 86400) snprintf(buf, n, "%ldh ago", d / 3600);
    else snprintf(buf, n, "%ldd ago", d / 86400);
}

static void fmt_dur(char *buf, size_t n, long s)
{
    if (s >= 86400) snprintf(buf, n, "%ldd %ldh", s / 86400, (s / 3600) % 24);
    else if (s >= 3600) snprintf(buf, n, "%ldh %02ldm", s / 3600, (s / 60) % 60);
    else if (s >= 60) snprintf(buf, n, "%ldm", s / 60);
    else snprintf(buf, n, "%lds", s);
}

static void run_cb(lv_event_t *e)
{
    (void)e;
    if (g_net.testing) return;
    hub_server_send("{\"type\":\"speedtest\"}");
    lv_label_set_text(s_run_lbl, "Starting...");
}

static lv_obj_t *top_card(lv_obj_t *page, int i, const char *caption, lv_obj_t **cap)
{
    lv_obj_t *c = ui_card(page);
    lv_obj_set_size(c, CARD_W, TOP_H);
    lv_obj_set_pos(c, i * (CARD_W + 12), 0);
    *cap = ui_caption(c, caption);
    return c;
}

static lv_obj_t *line(lv_obj_t *parent, const lv_font_t *font, uint32_t color)
{
    lv_obj_t *l = ui_label(parent, font, color, "");
    ui_marquee(l);
    lv_obj_set_width(l, CARD_W - 32);
    return l;
}

void net_page_create(lv_obj_t *page)
{
    // Each card: caption at the top, its last line on the bottom edge.
    lv_obj_t *cap;
    lv_obj_t *c = top_card(page, 0, "LATENCY", &cap);
    s_lat = line(c, UI_COMPACT ? g_font_32 : g_font_48, COL_TEXT);
    s_lat_sub = line(c, g_font_14, COL_MUTED);
    ui_spread(c, (lv_obj_t *[]){cap, s_lat, s_lat_sub}, 3);

    c = top_card(page, 1, "STATUS", &cap);
    s_status = line(c, g_font_32, COL_GOOD);
    s_drop = line(c, g_font_14, COL_MUTED);   // uptime and the last drop
    ui_spread(c, (lv_obj_t *[]){cap, s_status, s_drop}, 3);

    c = top_card(page, 2, "DATA USED", &cap);
    // Label on the left, amount on the right, one row each. The amounts are
    // 20 px so "This month" and an amount like "412.0 GB" fit side by side.
    lv_obj_t *today = line(c, g_font_16, COL_MUTED);
    lv_label_set_text(today, "Today");
    lv_obj_t *month = line(c, g_font_16, COL_MUTED);
    lv_label_set_text(month, "This month");
    ui_spread(c, (lv_obj_t *[]){cap, today, month}, 3);
    s_today = line(c, g_font_20, COL_TEXT);
    lv_obj_set_style_text_align(s_today, LV_TEXT_ALIGN_RIGHT, 0);
    lv_obj_align_to(s_today, today, LV_ALIGN_RIGHT_MID, 0, 0);
    s_month = line(c, g_font_20, COL_TEXT);
    lv_obj_set_style_text_align(s_month, LV_TEXT_ALIGN_RIGHT, 0);
    lv_obj_align_to(s_month, month, LV_ALIGN_RIGHT_MID, 0, 0);

    // Latency over the last two minutes.
    lv_obj_t *ch = ui_card(page);
    lv_obj_set_size(ch, CONTENT_W, CHART_H);
    lv_obj_set_pos(ch, 0, TOP_H + 12);
    s_chart_cap = ui_caption(ch, "LATENCY, LAST 2 MINUTES");
    lv_obj_set_pos(s_chart_cap, 0, 0);
    s_chart_max = ui_label(ch, g_font_14, COL_DIM, "");
    lv_obj_align(s_chart_max, LV_ALIGN_TOP_RIGHT, 0, 0);
    s_chart = lv_chart_create(ch);
    lv_obj_set_size(s_chart, CONTENT_W - 32, CHART_H - 32 - 24);
    lv_obj_set_pos(s_chart, 0, 24);
    lv_chart_set_type(s_chart, LV_CHART_TYPE_LINE);
    lv_chart_set_point_count(s_chart, HUB_NET_HIST);
    lv_chart_set_div_line_count(s_chart, 3, 0);
    lv_obj_set_style_line_color(s_chart, lv_color_hex(COL_CARD_HI), LV_PART_MAIN);
    lv_obj_set_style_bg_opa(s_chart, LV_OPA_TRANSP, 0);
    lv_obj_set_style_border_width(s_chart, 0, 0);
    lv_obj_set_style_pad_all(s_chart, 0, 0);
    lv_obj_set_style_size(s_chart, 0, 0, LV_PART_INDICATOR);   // no point dots
    lv_obj_set_style_line_width(s_chart, 2, LV_PART_ITEMS);
    s_ser = lv_chart_add_series(s_chart, lv_color_hex(COL_CYAN), LV_CHART_AXIS_PRIMARY_Y);

    // Speed test.
    lv_obj_t *b = ui_card(page);
    lv_obj_set_size(b, CONTENT_W, BOT_H);
    lv_obj_set_pos(b, 0, TOP_H + CHART_H + 24);
    cap = ui_caption(b, "SPEED TEST");
    lv_obj_set_pos(cap, 0, 0);
    lv_obj_t *dl = ui_label(b, g_icon_32, COL_CYAN, ICON_ARROW_DOWNWARD);
    lv_obj_set_pos(dl, 0, UI_COMPACT ? 20 : 26);
    s_down = ui_label(b, g_font_32, COL_TEXT, "");
    lv_obj_set_pos(s_down, 36, UI_COMPACT ? 18 : 24);
    lv_obj_t *ul = ui_label(b, g_icon_32, COL_ACCENT2, ICON_ARROW_UPWARD);
    lv_obj_set_pos(ul, 230, UI_COMPACT ? 20 : 26);
    s_upl = ui_label(b, g_font_32, COL_TEXT, "");
    lv_obj_set_pos(s_upl, 266, UI_COMPACT ? 18 : 24);
    s_sp_sub = ui_label(b, g_font_14, COL_MUTED, "");
    lv_obj_set_pos(s_sp_sub, 0, UI_COMPACT ? 54 : 66);
    s_now = ui_label(b, g_font_14, COL_DIM, "");
    lv_obj_align(s_now, LV_ALIGN_TOP_RIGHT, 0, 0);

    s_run = lv_button_create(b);
    lv_obj_remove_style_all(s_run);
    lv_obj_set_size(s_run, 150, UI_COMPACT ? 40 : 48);
    lv_obj_set_style_radius(s_run, UI_COMPACT ? 20 : 24, 0);
    lv_obj_set_style_bg_color(s_run, lv_color_hex(COL_ACCENT), 0);
    lv_obj_set_style_bg_opa(s_run, LV_OPA_COVER, 0);
    lv_obj_set_style_bg_color(s_run, lv_color_hex(COL_ACCENT_PRESSED), LV_STATE_PRESSED);
    lv_obj_set_style_bg_color(s_run, lv_color_hex(COL_CARD_HI), LV_STATE_DISABLED);
    lv_obj_set_ext_click_area(s_run, 8);
    lv_obj_align(s_run, LV_ALIGN_BOTTOM_RIGHT, 0, 0);
    lv_obj_add_event_cb(s_run, run_cb, LV_EVENT_CLICKED, NULL);
    s_run_lbl = ui_label(s_run, g_font_16, COL_BG, "Run test");
    lv_obj_center(s_run_lbl);

    s_offline = ui_label(ch, g_font_16, COL_DIM, "Start the companion app on your PC");
    lv_obj_align(s_offline, LV_ALIGN_CENTER, 0, 10);
    net_refresh();
}

void net_refresh_live(void)
{
    if (!s_now) return;
    // Live throughput from the PC's own counters (the stats message).
    char d[16], u[16], buf[80];
    snprintf(d, sizeof(d), "%.1f", g_stats.net_down * 8.192f / 1000);
    snprintf(u, sizeof(u), "%.1f", g_stats.net_up * 8.192f / 1000);
    snprintf(buf, sizeof(buf), "Now " "↓" " %s  " "↑" " %s Mbps", d, u);
    lv_label_set_text(s_now, g_pc_online ? buf : "");
}

void net_refresh(void)
{
    if (!s_lat) return;
    const hub_net_t *n = &g_net;
    char buf[112], t[24];
    if (g_pc_online) lv_obj_add_flag(s_offline, LV_OBJ_FLAG_HIDDEN);
    else lv_obj_remove_flag(s_offline, LV_OBJ_FLAG_HIDDEN);

    if (!n->valid) {
        lv_label_set_text(s_lat, "--");
        lv_label_set_text(s_status, "--");
        lv_obj_add_flag(s_run, LV_OBJ_FLAG_HIDDEN);
        return;
    }
    lv_obj_remove_flag(s_run, LV_OBJ_FLAG_HIDDEN);

    if (n->latency_ms >= 0) snprintf(buf, sizeof(buf), "%d ms", (int)(n->latency_ms + 0.5f));
    else strlcpy(buf, "--", sizeof(buf));
    lv_label_set_text(s_lat, buf);
    lv_obj_set_style_text_color(s_lat, lv_color_hex(n->latency_ms < 0 ? COL_DIM : n->latency_ms < 40 ? COL_TEXT
                                                    : n->latency_ms < 100 ? COL_WARN : COL_BAD), 0);
    snprintf(buf, sizeof(buf), "Jitter %.1f ms  " "•" "  Loss %d%%", n->jitter_ms, n->loss_pct);
    lv_label_set_text(s_lat_sub, buf);
    lv_obj_set_style_text_color(s_lat_sub, lv_color_hex(n->loss_pct >= 5 ? COL_BAD : COL_MUTED), 0);

    lv_label_set_text(s_status, n->online ? "Online" : "Offline");
    lv_obj_set_style_text_color(s_status, lv_color_hex(n->online ? COL_GOOD : COL_BAD), 0);
    // One line under the status: uptime, then the last drop (or the IP address).
    char up[32] = "", drop[64];
    if (n->online && n->up_since > 0) {
        fmt_dur(t, sizeof(t), (long)(time(NULL) - n->up_since));
        snprintf(up, sizeof(up), "Up %s", t);
    }
    if (n->drop_at > 0) {
        char ago[24];
        ui_fmt_ago(ago, sizeof(ago), n->drop_at);
        fmt_dur(t, sizeof(t), n->drop_dur_s);
        snprintf(drop, sizeof(drop), "Last drop %s, %s", ago, t);
    } else {
        strlcpy(drop, n->ip[0] ? n->ip : "No drops seen", sizeof(drop));
    }
    if (up[0]) snprintf(buf, sizeof(buf), "%s  " "•" "  %s", up, drop);
    else strlcpy(buf, drop, sizeof(buf));
    lv_label_set_text(s_drop, buf);

    ui_fmt_bytes(buf, sizeof(buf), n->today_bytes);
    lv_label_set_text(s_today, buf);
    ui_fmt_bytes(buf, sizeof(buf), n->month_bytes);
    lv_label_set_text(s_month, buf);

    // Chart: scale to the worst sample (at least 50 ms); lost probes show as gaps.
    int peak = 50;
    for (int i = 0; i < n->hist_count; i++) if (n->hist[i] > peak) peak = n->hist[i];
    lv_chart_set_range(s_chart, LV_CHART_AXIS_PRIMARY_Y, 0, peak);
    // The app's saved history (hist_label set) or an older app's last 2 minutes.
    char cap[48];
    snprintf(cap, sizeof(cap), "LATENCY, %.23s", n->hist_label[0] ? n->hist_label : "LAST 2 MINUTES");
    lv_label_set_text(s_chart_cap, cap);
    int points = n->hist_count > 0 ? n->hist_count : HUB_NET_HIST;
    if ((int)lv_chart_get_point_count(s_chart) != points) lv_chart_set_point_count(s_chart, points);
    for (int i = 0; i < points; i++) {
        int j = i - (points - n->hist_count);
        int v = j >= 0 && n->hist[j] >= 0 ? n->hist[j] : LV_CHART_POINT_NONE;
        lv_chart_set_value_by_id(s_chart, s_ser, i, v);
    }
    lv_chart_refresh(s_chart);
    snprintf(buf, sizeof(buf), "max %d ms", peak);
    lv_label_set_text(s_chart_max, buf);

    if (n->has_speed) {
        snprintf(buf, sizeof(buf), "%.0f Mbps", n->down_mbps);
        lv_label_set_text(s_down, buf);
        snprintf(buf, sizeof(buf), "%.0f Mbps", n->up_mbps);
        lv_label_set_text(s_upl, buf);
        ui_fmt_ago(t, sizeof(t), n->speed_at);
        snprintf(buf, sizeof(buf), "Ping %.0f ms  " "•" "  tested %s", n->speed_ping_ms, t);
        lv_label_set_text(s_sp_sub, buf);
    } else {
        lv_label_set_text(s_down, "--");
        lv_label_set_text(s_upl, "--");
        lv_label_set_text(s_sp_sub, "No test yet. A test uses about 35 MB of data.");
    }
    lv_label_set_text(s_run_lbl, n->testing ? "Testing..." : "Run test");
    if (n->testing) lv_obj_add_state(s_run, LV_STATE_DISABLED);
    else lv_obj_remove_state(s_run, LV_STATE_DISABLED);
    net_refresh_live();
}

void ui_set_net(const hub_net_t *n)
{
    g_net = *n;
    net_refresh();
    home_refresh_net();
}
