// "An agent is waiting for you" alert. The companion sends {"type":"alert",...}
// when a Claude Code session needs a permission approval or a reply, and
// {"type":"alert","clear":true} once nothing is waiting.
//
// It doesn't take over the screen: it covers the usage card on Home and the
// history card on the AI page, and puts a dot on the AI tab so it can be seen
// from any page.
#include <stdio.h>
#include <string.h>

#include "display.h"
#include "hub_server.h"
#include "icons.h"
#include "ui.h"
#include "ui_internal.h"

typedef struct {
    lv_obj_t *root, *logo, *kind, *project, *title, *detail;
    lv_obj_t *show, *later;   // Show on PC (hidden for a server's session), dismiss
} panel_t;

static panel_t s_home, s_ai;
static char s_id[64];
static hub_alert_t s_last;   // the alert on screen, to lay out again after Home moves
static bool s_active;

static void send_action(const char *action)
{
    char buf[128];
    snprintf(buf, sizeof(buf), "{\"type\":\"alert_action\",\"id\":\"%s\",\"action\":\"%s\"}", s_id, action);
    hub_server_send(buf);
}

static void hide(void)
{
    s_active = false;
    s_id[0] = 0;
    if (s_home.root) lv_obj_add_flag(s_home.root, LV_OBJ_FLAG_HIDDEN);
    if (s_ai.root) lv_obj_add_flag(s_ai.root, LV_OBJ_FLAG_HIDDEN);
    ui_rail_set_badge(PAGE_CLAUDE, 0);
}

static void on_focus(lv_event_t *e)
{
    (void)e;
    send_action("focus");
    hide();
}

static void on_dismiss(lv_event_t *e)
{
    (void)e;
    send_action("dismiss");
    hide();
}

static lv_obj_t *round_btn(lv_obj_t *parent, int size, bool primary, const char *glyph, lv_event_cb_t cb)
{
    lv_obj_t *b = ui_round_button(parent, size, primary ? COL_ACCENT : COL_CARD_HI, g_icon_24, glyph, cb, NULL);
    if (primary) lv_obj_set_style_text_color(lv_obj_get_child(b, 0), lv_color_hex(COL_BG), 0);
    return b;
}

static lv_obj_t *pill(lv_obj_t *parent, const char *text, bool primary, lv_event_cb_t cb)
{
    lv_obj_t *b = lv_button_create(parent);
    lv_obj_remove_style_all(b);
    lv_obj_set_size(b, 168, 46);
    lv_obj_set_style_radius(b, 23, 0);
    lv_obj_set_style_bg_color(b, lv_color_hex(primary ? COL_ACCENT : COL_CARD_HI), 0);
    lv_obj_set_style_bg_opa(b, LV_OPA_COVER, 0);
    lv_obj_set_style_border_color(b, lv_color_hex(COL_BORDER), 0);
    lv_obj_set_style_border_width(b, primary ? 0 : 1, 0);
    lv_obj_set_style_bg_color(b, lv_color_hex(COL_ACCENT_PRESSED), LV_STATE_PRESSED);
    lv_obj_set_ext_click_area(b, 6);
    lv_obj_t *l = ui_label(b, g_font_20, primary ? COL_BG : COL_TEXT, text);
    lv_obj_center(l);
    lv_obj_add_event_cb(b, cb, LV_EVENT_CLICKED, NULL);
    return b;
}

// An opaque layer over the whole host card, border and padding included.
static lv_obj_t *cover(lv_obj_t *host)
{
    lv_obj_t *p = lv_obj_create(host);
    lv_obj_remove_style_all(p);
    lv_obj_add_flag(p, LV_OBJ_FLAG_IGNORE_LAYOUT | LV_OBJ_FLAG_CLICKABLE);   // taps stop here
    lv_obj_remove_flag(p, LV_OBJ_FLAG_SCROLLABLE);
    lv_obj_set_style_bg_color(p, lv_color_hex(COL_CARD), 0);
    lv_obj_set_style_bg_opa(p, LV_OPA_COVER, 0);
    lv_obj_set_style_radius(p, lv_obj_get_style_radius(host, 0), 0);
    lv_obj_set_style_border_color(p, lv_color_hex(COL_WARN), 0);
    lv_obj_set_style_border_width(p, 2, 0);
    lv_obj_set_user_data(p, host);   // fit() may move it onto the page; it still follows this card
    return p;
}

// Match the host's current size (Home widgets are re-laid out as columns change).
#define ALERT_MIN_W 320   // narrower than this, the text has no room

static void fit(lv_obj_t *p)
{
    lv_obj_t *host = lv_obj_get_user_data(p);
    lv_obj_update_layout(host);
    int w = lv_obj_get_width(host);
    // (A hidden card keeps the strip as its child, so the strip hides with it.)
    if (w < ALERT_MIN_W && lv_obj_get_parent(host) && !lv_obj_has_flag(host, LV_OBJ_FLAG_HIDDEN)) {
        // A one-cell Home card: the strip sits on the page instead, wider than the
        // card and over its neighbours, but still on the page.
        lv_obj_t *page = lv_obj_get_parent(host);
        if (lv_obj_get_parent(p) != page) lv_obj_set_parent(p, page);
        int x = lv_obj_get_x(host);
        if (x + ALERT_MIN_W > CONTENT_W) x = CONTENT_W - ALERT_MIN_W;
        lv_obj_set_size(p, ALERT_MIN_W, lv_obj_get_height(host));
        lv_obj_set_pos(p, x, lv_obj_get_y(host));
        return;
    }
    if (lv_obj_get_parent(p) != host) lv_obj_set_parent(p, host);
    lv_obj_set_size(p, w, lv_obj_get_height(host));
    lv_obj_set_pos(p, -lv_obj_get_style_pad_left(host, 0) - lv_obj_get_style_border_width(host, 0),
                   -lv_obj_get_style_pad_top(host, 0) - lv_obj_get_style_border_width(host, 0));
}

// Home: a compact strip in the usage card's place.
static void build_home(lv_obj_t *host)
{
    panel_t *p = &s_home;
    p->root = cover(host);
    lv_obj_set_style_pad_hor(p->root, 14, 0);

    lv_obj_t *badge = lv_obj_create(p->root);
    lv_obj_remove_style_all(badge);
    lv_obj_set_size(badge, 40, 40);
    lv_obj_set_style_radius(badge, 12, 0);
    lv_obj_set_style_bg_color(badge, lv_color_hex(COL_CARD_HI), 0);
    lv_obj_set_style_bg_opa(badge, LV_OPA_COVER, 0);
    lv_obj_align(badge, LV_ALIGN_LEFT_MID, 0, 0);
    p->logo = lv_image_create(badge);
    lv_obj_center(p->logo);

    p->kind = ui_label(p->root, g_font_14, COL_WARN, "");
    ui_marquee(p->kind);
    lv_obj_set_height(p->kind, 18);   // one line, cut with "..."
    lv_obj_align(p->kind, LV_ALIGN_LEFT_MID, 52, -12);
    p->detail = ui_label(p->root, g_font_16, COL_TEXT, "");
    ui_marquee(p->detail);
    lv_obj_set_height(p->detail, 20);
    lv_obj_align(p->detail, LV_ALIGN_LEFT_MID, 52, 12);

    p->show = round_btn(p->root, 40, true, ICON_OPEN_IN_NEW, on_focus);
    lv_obj_align(p->show, LV_ALIGN_RIGHT_MID, 0, 0);
    p->later = round_btn(p->root, 40, false, ICON_CLOSE, on_dismiss);
    lv_obj_align(p->later, LV_ALIGN_RIGHT_MID, -48, 0);
}

// AI page: a panel over the 7-day history card. Everything is left-aligned: a
// header (logo, kind, project), the message, an optional second line, and the
// buttons on their own row at the bottom so the text never runs under them.
static void build_ai(lv_obj_t *host)
{
    panel_t *p = &s_ai;
    p->root = cover(host);
    lv_obj_set_style_pad_all(p->root, 18, 0);

    lv_obj_t *badge = lv_obj_create(p->root);
    lv_obj_remove_style_all(badge);
    lv_obj_set_size(badge, 40, 40);
    lv_obj_set_style_radius(badge, 12, 0);
    lv_obj_set_style_bg_color(badge, lv_color_hex(COL_CARD_HI), 0);
    lv_obj_set_style_bg_opa(badge, LV_OPA_COVER, 0);
    lv_obj_align(badge, LV_ALIGN_TOP_LEFT, 0, 0);
    p->logo = lv_image_create(badge);
    lv_obj_center(p->logo);

    p->kind = ui_caption(p->root, "");
    lv_obj_align(p->kind, LV_ALIGN_TOP_LEFT, 52, 0);
    p->project = ui_label(p->root, g_font_14, COL_MUTED, "");
    ui_marquee(p->project);
    lv_obj_align(p->project, LV_ALIGN_TOP_LEFT, 52, 21);

    p->title = ui_label(p->root, g_font_24, COL_TEXT, "");
    ui_marquee(p->title);
    lv_obj_align(p->title, LV_ALIGN_TOP_LEFT, 0, 54);
    p->detail = ui_label(p->root, g_font_16, COL_MUTED, "");
    ui_marquee(p->detail);
    lv_obj_set_height(p->detail, 20);
    lv_obj_align(p->detail, LV_ALIGN_TOP_LEFT, 0, 86);

    p->show = pill(p->root, "Show on PC", true, on_focus);
    lv_obj_align(p->show, LV_ALIGN_BOTTOM_RIGHT, 0, 0);
    p->later = pill(p->root, "Not now", false, on_dismiss);
    lv_obj_align(p->later, LV_ALIGN_BOTTOM_RIGHT, -180, 0);
}

// The Home strip has room for one short line: drop the leading "Claude " and
// "Finished. " from the companion's text ("Claude wants to use Bash" -> "Wants to use Bash").
static const char *short_detail(const hub_alert_t *a)
{
    static char buf[128];
    const char *s = a->detail[0] ? a->detail : a->title;
    if (strncmp(s, "Claude ", 7) == 0) s += 7;
    if (strncmp(s, "Finished. ", 10) == 0) s += 10;
    strlcpy(buf, s, sizeof(buf));
    if (buf[0] >= 'a' && buf[0] <= 'z') buf[0] -= 'a' - 'A';
    return buf;
}

static void fill(panel_t *p, const hub_alert_t *a, bool compact)
{
    if (!p->root) return;
    bool perm = strcmp(a->kind, "permission") == 0;
    const char *kind = perm ? "NEEDS PERMISSION" : "WAITING FOR YOU";
    lv_image_set_src(p->logo, ui_agent_logo(24));
    fit(p->root);
    lv_obj_update_layout(p->root);   // so the width below is the new one
    int inner = lv_obj_get_width(p->root) - 2 * lv_obj_get_style_pad_left(p->root, 0) - 4;

    if (compact) {
        // The project on top; the colour (amber/periwinkle) says permission or reply.
        lv_label_set_text(p->kind, a->project[0] ? a->project : kind);
        lv_label_set_text(p->detail, short_detail(a));
        int w = inner - 52 - 2 * 40 - 16;   // between the logo and the buttons
        lv_obj_set_width(p->kind, w);
        lv_obj_set_width(p->detail, w);
    } else {
        lv_label_set_text(p->kind, kind);
        lv_label_set_text(p->project, a->project);
        const char *title = a->title[0] ? a->title : "Claude is waiting for you";
        lv_label_set_text(p->title, title);
        // A second line only when it adds something (older companions sent the
        // same message as both).
        lv_label_set_text(p->detail, strcmp(a->detail, title) == 0 ? "" : a->detail);
        lv_obj_set_width(p->project, inner - 52);
        lv_obj_set_width(p->title, inner);
        lv_obj_set_width(p->detail, inner);
    }
    // A server's session has no window on the PC: no Show, dismiss takes its place.
    if (a->no_focus) lv_obj_add_flag(p->show, LV_OBJ_FLAG_HIDDEN);
    else lv_obj_remove_flag(p->show, LV_OBJ_FLAG_HIDDEN);
    lv_obj_align(p->later, compact ? LV_ALIGN_RIGHT_MID : LV_ALIGN_BOTTOM_RIGHT,
                 a->no_focus ? 0 : (compact ? -48 : -180), 0);
    lv_obj_set_style_text_color(p->kind, lv_color_hex(perm ? COL_WARN : COL_ACCENT), 0);
    lv_obj_set_style_border_color(p->root, lv_color_hex(perm ? COL_WARN : COL_ACCENT), 0);
    lv_obj_remove_flag(p->root, LV_OBJ_FLAG_HIDDEN);
    lv_obj_move_foreground(p->root);
}

void ui_set_alert(const hub_alert_t *a)
{
    if (!a->active) {
        hide();
        return;
    }
    if (!s_home.root && home_claude_card()) build_home(home_claude_card());
    if (!s_ai.root && claude_history_card()) build_ai(claude_history_card());

    bool is_new = !s_active || strcmp(s_id, a->id) != 0;
    strlcpy(s_id, a->id, sizeof(s_id));
    if (a != &s_last) s_last = *a;
    s_active = true;
    fill(&s_home, a, true);
    fill(&s_ai, a, false);
    ui_rail_set_badge(PAGE_CLAUDE, strcmp(a->kind, "permission") == 0 ? COL_WARN : COL_ACCENT);
    if (is_new) display_wake();
}

// Home re-lays out its widgets (e.g. when the clock width changes); keep the
// strip the same size as the card under it.
void ui_alert_relayout(void)
{
    if (s_active && s_home.root) fill(&s_home, &s_last, true);
}
