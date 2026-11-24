// GitHub: the contribution chart (shared with the Home widget) and the GitHub page.
//
// The page: the whole year's chart with month and weekday labels across the top,
// and three cards under it: the streak, this week against last week day by day,
// and the repositories with the most commits this year.
#include <stdio.h>
#include <string.h>

#include "ui.h"
#include "ui_internal.h"

hub_github_t g_github;

// ---- the chart ---------------------------------------------------------------
// One square per day, a column per week with Sunday on top, the newest week on
// the right. The object is 1 px bigger than the squares all round so today's
// outline (drawn in the gap around its square) isn't clipped.

typedef struct {
    int x, y;       // top left of the first square, in the parent
    int pitch;      // square + gap
    int weeks;      // columns shown
} gh_chart_t;

int github_total_weeks(void)
{
    const hub_github_t *g = &g_github;
    return g->day_count > 0 ? (g->day_count - 1 + g->start_wday) / 7 + 1 : 53;
}

static uint32_t level_color(int level)
{
    static const uint32_t green[5] = {0, 0x0e4429, 0x006d32, 0x26a641, 0x39d353};
    if (level <= 0 || level > 4) return COL_CARD_HI;
    if (!g_github.accent) return green[level];
    // Shades of the accent over the empty square: 30%, 55%, 80%, 100%.
    static const uint8_t mix[5] = {0, 77, 140, 204, 255};
    lv_color_t c = lv_color_mix(lv_color_hex(COL_ACCENT), lv_color_hex(COL_CARD_HI), mix[level]);
    return lv_color_to_u32(c) & 0xffffff;
}

static uint32_t bar_color(void)
{
    return g_github.accent ? COL_ACCENT : 0x26a641;
}

static void chart_draw_cb(lv_event_t *e)
{
    lv_obj_t *obj = lv_event_get_target(e);
    const gh_chart_t *st = lv_obj_get_user_data(obj);
    if (!st || st->weeks <= 0) return;
    lv_layer_t *layer = lv_event_get_layer(e);
    lv_area_t box;
    lv_obj_get_coords(obj, &box);
    const hub_github_t *g = &g_github;
    bool have = g->day_count > 0;
    int sq = st->pitch - GH_GAP;
    int first = github_total_weeks() - st->weeks;   // oldest week column shown
    lv_draw_rect_dsc_t d;
    lv_draw_rect_dsc_init(&d);
    d.radius = sq >= 10 ? 2 : 1;
    d.bg_opa = LV_OPA_COVER;
    int days = have ? g->day_count : st->weeks * 7;
    int start = have ? g->start_wday : 0;
    for (int i = 0; i < days; i++) {
        int col = (i + start) / 7 - (have ? first : 0);
        if (col < 0) continue;
        int row = (i + start) % 7;
        lv_area_t a;
        a.x1 = box.x1 + 1 + col * st->pitch;
        a.y1 = box.y1 + 1 + row * st->pitch;
        a.x2 = a.x1 + sq - 1;
        a.y2 = a.y1 + sq - 1;
        d.bg_color = lv_color_hex(level_color(have ? g->level[i] : 0));
        lv_draw_rect(layer, &d, &a);
        if (have && i == g->day_count - 1) {
            // Today: an outline in the gap around the square, so its colour still shows.
            lv_draw_rect_dsc_t o;
            lv_draw_rect_dsc_init(&o);
            o.radius = d.radius + 1;
            o.bg_opa = LV_OPA_TRANSP;
            o.border_width = 1;
            o.border_color = lv_color_hex(COL_TEXT);
            o.border_opa = LV_OPA_COVER;
            lv_area_t r = {a.x1 - 1, a.y1 - 1, a.x2 + 1, a.y2 + 1};
            lv_draw_rect(layer, &o, &r);
        }
    }
}

static void chart_delete_cb(lv_event_t *e)
{
    lv_free(lv_obj_get_user_data(lv_event_get_target(e)));
}

lv_obj_t *github_chart_create(lv_obj_t *parent)
{
    lv_obj_t *c = lv_obj_create(parent);
    lv_obj_remove_style_all(c);
    lv_obj_remove_flag(c, LV_OBJ_FLAG_SCROLLABLE | LV_OBJ_FLAG_CLICKABLE);
    gh_chart_t *st = lv_malloc_zeroed(sizeof(gh_chart_t));
    if (st) st->pitch = 12;
    lv_obj_set_user_data(c, st);
    lv_obj_add_event_cb(c, chart_draw_cb, LV_EVENT_DRAW_MAIN, NULL);
    lv_obj_add_event_cb(c, chart_delete_cb, LV_EVENT_DELETE, NULL);
    return c;
}

void github_chart_set(lv_obj_t *chart, int x, int y, int pitch, int weeks)
{
    gh_chart_t *st = lv_obj_get_user_data(chart);
    if (!st) return;
    st->x = x;
    st->y = y;
    st->pitch = pitch;
    st->weeks = weeks;
    lv_obj_set_pos(chart, x - 1, y - 1);
    lv_obj_set_size(chart, weeks * pitch - GH_GAP + 2, 7 * pitch - GH_GAP + 2);
    lv_obj_invalidate(chart);
}

void github_place_months(lv_obj_t *const *labels, lv_obj_t *chart, int y, bool show)
{
    const hub_github_t *g = &g_github;
    const gh_chart_t *st = lv_obj_get_user_data(chart);
    int first = st ? github_total_weeks() - st->weeks : 0, last_x = -1000;
    for (int i = 0; i < HUB_GH_MONTHS; i++) {
        lv_obj_t *l = labels[i];
        int col = i < g->month_count ? g->month_col[i] - first : -1;
        if (!show || !st || col < 0 || col >= st->weeks) {
            lv_obj_add_flag(l, LV_OBJ_FLAG_HIDDEN);
            continue;
        }
        int x = st->x + col * st->pitch;
        int tw = lv_text_get_width(g->month_name[i], strlen(g->month_name[i]), g_font_14, 0);
        // Skip a label that would crowd the one before or run past the last column.
        if (x < last_x + 6 || x + tw > st->x + st->weeks * st->pitch) {
            lv_obj_add_flag(l, LV_OBJ_FLAG_HIDDEN);
            continue;
        }
        lv_label_set_text(l, g->month_name[i]);
        lv_obj_set_pos(l, x, y);
        lv_obj_remove_flag(l, LV_OBJ_FLAG_HIDDEN);
        last_x = x + tw;
    }
}

void github_fmt_count(char *buf, size_t n, int v)
{
    if (v >= 1000000) snprintf(buf, n, "%d,%03d,%03d", v / 1000000, v / 1000 % 1000, v % 1000);
    else if (v >= 1000) snprintf(buf, n, "%d,%03d", v / 1000, v % 1000);
    else snprintf(buf, n, "%d", v);
}

// ---- the page ----------------------------------------------------------------

#define PG_GAP (UI_COMPACT ? 8 : 12)
#define WD_W 34   // the Mon / Wed / Fri column left of the chart
#define REPO_BAR_H 4

static lv_obj_t *s_top, *s_sum, *s_chart, *s_month[HUB_GH_MONTHS], *s_wday[3];
static lv_obj_t *s_st_card, *s_st_cap, *s_st_val, *s_st_unit, *s_st_row[2], *s_st_key[2], *s_st_txt[2];
static lv_obj_t *s_wk_card, *s_wk_cap, *s_wk_delta, *s_wk_val, *s_wk_bars, *s_wk_day[7];
static lv_obj_t *s_rp_card, *s_rp_cap, *s_rp_empty, *s_rp_name[HUB_GH_REPOS], *s_rp_num[HUB_GH_REPOS],
    *s_rp_bar[HUB_GH_REPOS];

static lv_obj_t *card(lv_obj_t *parent)
{
    lv_obj_t *c = ui_card(parent);
    if (UI_COMPACT) lv_obj_set_style_pad_ver(c, 9, 0);
    return c;
}

static int pad_v(void)
{
    return UI_COMPACT ? 9 : 16;
}

// A row with a muted name on the left and a value on the right edge.
static lv_obj_t *kv_row(lv_obj_t *parent, const char *key, lv_obj_t **key_out, lv_obj_t **val_out)
{
    lv_obj_t *r = lv_obj_create(parent);
    lv_obj_remove_style_all(r);
    lv_obj_remove_flag(r, LV_OBJ_FLAG_SCROLLABLE | LV_OBJ_FLAG_CLICKABLE);
    *key_out = ui_label(r, g_font_16, COL_MUTED, key);
    lv_obj_align(*key_out, LV_ALIGN_LEFT_MID, 0, 0);
    *val_out = ui_label(r, g_font_16, COL_TEXT, "--");
    lv_obj_align(*val_out, LV_ALIGN_RIGHT_MID, 0, 0);
    return r;
}

// This week (bright) beside last week (dim), a pair of bars per weekday.
static void week_draw_cb(lv_event_t *e)
{
    lv_obj_t *obj = lv_event_get_target(e);
    lv_layer_t *layer = lv_event_get_layer(e);
    lv_area_t box;
    lv_obj_get_coords(obj, &box);
    const hub_github_t *g = &g_github;
    int max = 1;
    for (int i = 0; i < 7; i++) {
        if (i < g->week_day_count && g->week_days[i] > max) max = g->week_days[i];
        if (g->last_week_days[i] > max) max = g->last_week_days[i];
    }
    int w = lv_area_get_width(&box), h = lv_area_get_height(&box);
    int slot = w / 7, bw = (slot - 10) / 2;
    if (bw < 3) bw = 3;
    lv_draw_rect_dsc_t d;
    lv_draw_rect_dsc_init(&d);
    d.radius = 2;
    d.bg_opa = LV_OPA_COVER;
    for (int i = 0; i < 7; i++) {
        int x = box.x1 + i * slot + (slot - 2 * bw - 2) / 2;
        for (int k = 0; k < 2; k++) {
            bool now = k == 1;
            if (now && i >= g->week_day_count) continue;   // still to come
            int v = now ? g->week_days[i] : g->last_week_days[i];
            int bh = v > 0 ? v * h / max : 0;
            if (bh < 2) bh = 2;   // a sliver marks an empty day
            lv_area_t a = {x + k * (bw + 2), box.y2 - bh + 1, x + k * (bw + 2) + bw - 1, box.y2};
            d.bg_color = lv_color_hex(now ? (v > 0 ? bar_color() : COL_CARD_HI) : COL_PRESSED);
            lv_draw_rect(layer, &d, &a);
        }
    }
}

void github_page_create(lv_obj_t *page)
{
    // The year.
    s_top = card(page);
    lv_obj_t *cap = ui_caption(s_top, "LAST YEAR");
    lv_obj_set_pos(cap, 0, 0);
    s_sum = ui_label(s_top, g_font_14, COL_MUTED, "");
    ui_marquee(s_sum);
    lv_obj_set_style_text_align(s_sum, LV_TEXT_ALIGN_RIGHT, 0);
    s_chart = github_chart_create(s_top);
    for (int i = 0; i < HUB_GH_MONTHS; i++) s_month[i] = ui_label(s_top, g_font_14, COL_DIM, "");
    static const char *const wd[3] = {"Mon", "Wed", "Fri"};
    for (int i = 0; i < 3; i++) s_wday[i] = ui_label(s_top, g_font_14, COL_DIM, wd[i]);

    // The streak.
    s_st_card = card(page);
    s_st_cap = ui_caption(s_st_card, "STREAK");
    s_st_val = ui_label(s_st_card, g_font_48, COL_TEXT, "--");
    s_st_unit = ui_label(s_st_card, g_font_20, COL_MUTED, "days");
    s_st_row[0] = kv_row(s_st_card, "Longest", &s_st_key[0], &s_st_txt[0]);
    s_st_row[1] = kv_row(s_st_card, "Best day", &s_st_key[1], &s_st_txt[1]);

    // This week against last week.
    s_wk_card = card(page);
    s_wk_cap = ui_caption(s_wk_card, "THIS WEEK");
    s_wk_delta = ui_label(s_wk_card, g_font_14, COL_MUTED, "");
    s_wk_val = ui_label(s_wk_card, g_font_32, COL_TEXT, "--");
    s_wk_bars = lv_obj_create(s_wk_card);
    lv_obj_remove_style_all(s_wk_bars);
    lv_obj_remove_flag(s_wk_bars, LV_OBJ_FLAG_SCROLLABLE | LV_OBJ_FLAG_CLICKABLE);
    lv_obj_add_event_cb(s_wk_bars, week_draw_cb, LV_EVENT_DRAW_MAIN, NULL);
    static const char *const days = "SMTWTFS";
    for (int i = 0; i < 7; i++) {
        char t[2] = {days[i], 0};
        s_wk_day[i] = ui_label(s_wk_card, g_font_14, COL_DIM, t);
    }

    // Top repositories.
    s_rp_card = card(page);
    s_rp_cap = ui_caption(s_rp_card, "TOP REPOS");
    s_rp_empty = ui_label(s_rp_card, g_font_16, COL_MUTED, "");
    for (int i = 0; i < HUB_GH_REPOS; i++) {
        s_rp_name[i] = ui_label(s_rp_card, g_font_16, COL_TEXT, "");
        ui_marquee(s_rp_name[i]);
        s_rp_num[i] = ui_label(s_rp_card, g_font_16, COL_MUTED, "");
        s_rp_bar[i] = lv_obj_create(s_rp_card);
        lv_obj_remove_style_all(s_rp_bar[i]);
        lv_obj_set_style_radius(s_rp_bar[i], 2, 0);
        lv_obj_set_style_bg_opa(s_rp_bar[i], LV_OPA_COVER, 0);
        lv_obj_remove_flag(s_rp_bar[i], LV_OBJ_FLAG_SCROLLABLE | LV_OBJ_FLAG_CLICKABLE);
    }
    github_page_refresh();
}

// Positions depend on the data (weeks shown, label widths), so refresh lays out too.
static void layout_page(void)
{
    int lh14 = lv_font_get_line_height(g_font_14), lh16 = lv_font_get_line_height(g_font_16);
    int pv = pad_v();

    // Year card: caption row, month labels, then the chart with weekday labels to its left.
    int iw = CONTENT_W - 32;
    int pitch = (iw - WD_W + GH_GAP) / 53;
    if (pitch > 16) pitch = 16;
    int weeks = (iw - WD_W + GH_GAP) / pitch;
    if (weeks > github_total_weeks()) weeks = github_total_weeks();
    int month_y = lh14 + 6, chart_y = month_y + lh14 + 4;
    int chart_x = iw - (weeks * pitch - GH_GAP);   // flush right: the newest week on the edge
    int inner = chart_y + 7 * pitch - GH_GAP;
    int top_h = inner + 2 * pv + 2;
    lv_obj_set_pos(s_top, 0, 0);
    lv_obj_set_size(s_top, CONTENT_W, top_h);
    int cap_w = lv_text_get_width("LAST YEAR", 9, g_font_14, 2) + 16;
    lv_obj_set_size(s_sum, iw - cap_w, lh14);
    lv_obj_set_pos(s_sum, cap_w, 0);
    github_chart_set(s_chart, chart_x, chart_y, pitch, weeks);
    github_place_months(s_month, s_chart, month_y, true);
    for (int i = 0; i < 3; i++) {
        int row = 1 + 2 * i;
        lv_obj_set_pos(s_wday[i], chart_x - WD_W, chart_y + row * pitch + (pitch - GH_GAP) / 2 - lh14 / 2);
    }

    // Three cards under it, filling the rest of the page.
    int y = top_h + PG_GAP, h = CONTENT_H - y;
    int cw = (CONTENT_W - 2 * PG_GAP) / 3;
    lv_obj_t *cards[3] = {s_st_card, s_wk_card, s_rp_card};
    for (int i = 0; i < 3; i++) {
        lv_obj_set_pos(cards[i], i * (cw + PG_GAP), y);
        lv_obj_set_size(cards[i], i == 2 ? CONTENT_W - 2 * (cw + PG_GAP) : cw, h);
    }
    int ciw = cw - 32, cih = h - 2 * pv - 2;

    // Streak: caption, the big number, then two rows on the bottom edge.
    lv_obj_set_pos(s_st_cap, 0, 0);
    int lh48 = lv_font_get_line_height(g_font_48);
    int rows_y = cih - 2 * lh16 - 6;
    int big_y = lh14 + (rows_y - lh14 - lh48) / 2;
    lv_obj_set_pos(s_st_val, 0, big_y);
    lv_obj_update_layout(s_st_val);
    lv_obj_set_pos(s_st_unit, lv_obj_get_width(s_st_val) + 8, big_y + lh48 - lv_font_get_line_height(g_font_20) - 6);
    for (int i = 0; i < 2; i++) {
        lv_obj_set_size(s_st_row[i], ciw, lh16);
        lv_obj_set_pos(s_st_row[i], 0, cih - (2 - i) * lh16 - (1 - i) * 6);
        lv_obj_align(s_st_txt[i], LV_ALIGN_RIGHT_MID, 0, 0);
    }

    // This week: caption and change, the count, then the bars over the day letters.
    // The change from last week sits after the count, on its baseline.
    lv_obj_set_pos(s_wk_cap, 0, 0);
    int lh32 = lv_font_get_line_height(g_font_32);
    lv_obj_set_pos(s_wk_val, 0, lh14 + 2);
    lv_obj_update_layout(s_wk_val);
    lv_obj_set_pos(s_wk_delta, lv_obj_get_width(s_wk_val) + 10, lh14 + 2 + lh32 - lh14 - 5);
    int bars_y = lh14 + 2 + lh32 + 8, day_y = cih - lh14;
    lv_obj_set_pos(s_wk_bars, 0, bars_y);
    lv_obj_set_size(s_wk_bars, ciw, day_y - 4 - bars_y);
    lv_obj_invalidate(s_wk_bars);
    int slot = ciw / 7;
    for (int i = 0; i < 7; i++) {
        lv_obj_update_layout(s_wk_day[i]);
        lv_obj_set_pos(s_wk_day[i], i * slot + (slot - lv_obj_get_width(s_wk_day[i])) / 2, day_y);
    }

    // Repositories: a name and count over a thin bar each, rows spread down the card.
    int rciw = lv_obj_get_style_width(s_rp_card, 0) - 32;
    lv_obj_set_pos(s_rp_cap, 0, 0);
    lv_obj_set_pos(s_rp_empty, 0, lh14 + 12);
    int n = g_github.repo_count, row_h = lh16 + 4 + REPO_BAR_H;
    int top = lh14 + 10, span = cih - top;
    int max = n > 0 && g_github.repo_commits[0] > 0 ? g_github.repo_commits[0] : 1;
    for (int i = 0; i < HUB_GH_REPOS; i++) {
        bool on = i < n;
        lv_obj_t *objs[3] = {s_rp_name[i], s_rp_num[i], s_rp_bar[i]};
        for (int k = 0; k < 3; k++) {
            if (on) lv_obj_remove_flag(objs[k], LV_OBJ_FLAG_HIDDEN);
            else lv_obj_add_flag(objs[k], LV_OBJ_FLAG_HIDDEN);
        }
        if (!on) continue;
        // The first row under the caption, the last on the bottom edge.
        int ry = n > 1 ? top + i * (span - row_h) / (n - 1) : top;
        lv_obj_update_layout(s_rp_num[i]);
        int num_w = lv_obj_get_width(s_rp_num[i]);
        lv_obj_set_pos(s_rp_num[i], rciw - num_w, ry);
        lv_obj_set_size(s_rp_name[i], rciw - num_w - 10, lh16);
        lv_obj_set_pos(s_rp_name[i], 0, ry);
        int bw = g_github.repo_commits[i] * rciw / max;
        lv_obj_set_size(s_rp_bar[i], bw < 4 ? 4 : bw, REPO_BAR_H);
        lv_obj_set_pos(s_rp_bar[i], 0, ry + lh16 + 4);
        lv_obj_set_style_bg_color(s_rp_bar[i], lv_color_hex(bar_color()), 0);
    }
}

void github_page_refresh(void)
{
    if (!s_top) return;
    const hub_github_t *g = &g_github;
    char buf[160], a[16];
    if (!g->valid) {
        const char *why = g_pc_online ? "Waiting for the PC" : "PC offline";
        lv_label_set_text(s_sum, why);
        lv_label_set_text(s_st_val, "--");
        lv_label_set_text(s_st_txt[0], "--");
        lv_label_set_text(s_st_txt[1], "--");
        lv_label_set_text(s_wk_val, "--");
        lv_label_set_text(s_wk_delta, "");
        lv_label_set_text(s_rp_empty, why);
        lv_obj_remove_flag(s_rp_empty, LV_OBJ_FLAG_HIDDEN);
    } else {
        // "1,019 contributions  •  842 commits  •  120 private  •  61 PRs  •  40 reviews  •  12 issues"
        github_fmt_count(a, sizeof(a), g->total);
        int len = snprintf(buf, sizeof(buf), "%s contributions", a);
        const int nums[5] = {g->commits, g->private_count, g->prs, g->reviews, g->issues};
        static const char *const what[5] = {"commits", "private", "PRs", "reviews", "issues"};
        for (int i = 0; i < 5 && len < (int)sizeof(buf); i++) {
            if (nums[i] <= 0) continue;
            github_fmt_count(a, sizeof(a), nums[i]);
            len += snprintf(buf + len, sizeof(buf) - len, "  " "•" "  %s %s", a, what[i]);
        }
        lv_label_set_text(s_sum, buf);

        snprintf(buf, sizeof(buf), "%d", g->streak);
        lv_label_set_text(s_st_val, buf);
        lv_label_set_text(s_st_unit, g->streak == 1 ? "day" : "days");
        snprintf(buf, sizeof(buf), "%d day%s", g->longest, g->longest == 1 ? "" : "s");
        lv_label_set_text(s_st_txt[0], buf);
        if (g->best > 0) snprintf(buf, sizeof(buf), "%d on %.7s", g->best, g->best_date);
        else strlcpy(buf, "--", sizeof(buf));
        lv_label_set_text(s_st_txt[1], buf);

        github_fmt_count(a, sizeof(a), g->week);
        lv_label_set_text(s_wk_val, a);
        int diff = g->week - g->last_week;
        if (g->last_week == 0 && g->week == 0) buf[0] = 0;
        else if (diff == 0) strlcpy(buf, "same as last week", sizeof(buf));
        else snprintf(buf, sizeof(buf), "%+d vs last week", diff);
        lv_label_set_text(s_wk_delta, buf);
        lv_obj_set_style_text_color(s_wk_delta, lv_color_hex(diff > 0 ? COL_GOOD : COL_MUTED), 0);

        for (int i = 0; i < g->repo_count; i++) {
            lv_label_set_text(s_rp_name[i], g->repo_name[i]);
            github_fmt_count(a, sizeof(a), g->repo_commits[i]);
            lv_label_set_text(s_rp_num[i], a);
        }
        lv_label_set_text(s_rp_empty, "No commits this year");
        if (g->repo_count) lv_obj_add_flag(s_rp_empty, LV_OBJ_FLAG_HIDDEN);
        else lv_obj_remove_flag(s_rp_empty, LV_OBJ_FLAG_HIDDEN);
    }
    // Today's letter stands out.
    for (int i = 0; i < 7; i++) {
        bool today = g->valid && i == g->week_day_count - 1;
        lv_obj_set_style_text_color(s_wk_day[i], lv_color_hex(today ? COL_TEXT : COL_DIM), 0);
    }
    layout_page();
}

void ui_set_github(const hub_github_t *g)
{
    g_github = *g;
    home_refresh_github();
    github_page_refresh();
}
