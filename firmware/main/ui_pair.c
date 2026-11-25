// The pairing prompt: "Pair with <PC name>?" with Deny / Allow (pair.c waits for the answer).
// Looks like the firmware update prompt (ui_update.c).
#include <stdio.h>

#include "icons.h"
#include "pair.h"
#include "ui.h"
#include "ui_internal.h"

#define ICON_LINK "\xEE\x85\x97"   /* e157 link */

static lv_obj_t *s_root, *s_title, *s_code, *s_from;

static void answer_cb(lv_event_t *e)
{
    pair_answer((bool)(intptr_t)lv_event_get_user_data(e));
}

static lv_obj_t *button(lv_obj_t *parent, const char *text, bool primary)
{
    lv_obj_t *b = lv_button_create(parent);
    lv_obj_remove_style_all(b);
    lv_obj_set_size(b, 180, 56);
    lv_obj_set_style_radius(b, 28, 0);
    lv_obj_set_style_bg_opa(b, LV_OPA_COVER, 0);
    lv_obj_set_style_bg_color(b, lv_color_hex(primary ? COL_ACCENT : COL_CARD_HI), 0);
    lv_obj_set_style_bg_color(b, lv_color_hex(primary ? COL_ACCENT_PRESSED : COL_PRESSED), LV_STATE_PRESSED);
    lv_obj_set_ext_click_area(b, 8);
    lv_obj_t *l = ui_label(b, g_font_20, primary ? COL_ON_ACCENT : COL_TEXT, text);
    lv_obj_center(l);
    lv_obj_add_event_cb(b, answer_cb, LV_EVENT_CLICKED, (void *)(intptr_t)primary);
    return b;
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

    lv_obj_t *card = ui_card(s_root);
    lv_obj_set_size(card, 560, 340);
    lv_obj_center(card);
    lv_obj_set_style_pad_all(card, 28, 0);
    lv_obj_set_flex_flow(card, LV_FLEX_FLOW_COLUMN);
    lv_obj_set_flex_align(card, LV_FLEX_ALIGN_START, LV_FLEX_ALIGN_CENTER, LV_FLEX_ALIGN_CENTER);
    lv_obj_set_style_pad_row(card, 10, 0);

    ui_label(card, g_icon_48, COL_ACCENT, ICON_LINK);
    s_title = ui_label(card, g_font_24, COL_TEXT, "");
    lv_obj_set_width(s_title, 500);
    lv_obj_set_style_text_align(s_title, LV_TEXT_ALIGN_CENTER, 0);
    lv_label_set_long_mode(s_title, LV_LABEL_LONG_DOT);
    s_code = ui_label(card, g_font_32, COL_ACCENT, "");
    lv_obj_set_style_text_letter_space(s_code, 6, 0);
    s_from = ui_label(card, g_font_16, COL_MUTED, "");
    lv_obj_t *sub = ui_label(card, g_font_16, COL_MUTED,
                             "Allow only if the Desk Hub app shows this same code.");
    lv_obj_set_width(sub, 500);
    lv_obj_set_style_text_align(sub, LV_TEXT_ALIGN_CENTER, 0);

    lv_obj_t *buttons = lv_obj_create(card);
    lv_obj_remove_style_all(buttons);
    lv_obj_set_size(buttons, LV_SIZE_CONTENT, LV_SIZE_CONTENT);
    lv_obj_set_flex_flow(buttons, LV_FLEX_FLOW_ROW);
    lv_obj_set_style_pad_column(buttons, 20, 0);
    lv_obj_set_style_margin_top(buttons, 14, 0);
    button(buttons, "Deny", false);
    button(buttons, "Allow", true);
}

void ui_pair_show(const char *name, const char *code, const char *ip)
{
    if (!s_root) build();
    char buf[64];
    snprintf(buf, sizeof(buf), "Pair with %s?", name);
    lv_label_set_text(s_title, buf);
    lv_label_set_text(s_code, code);
    snprintf(buf, sizeof(buf), "Asked from %s", ip);
    lv_label_set_text(s_from, buf);
    lv_obj_remove_flag(s_root, LV_OBJ_FLAG_HIDDEN);
    lv_obj_move_foreground(s_root);
}

void ui_pair_hide(void)
{
    if (s_root) lv_obj_add_flag(s_root, LV_OBJ_FLAG_HIDDEN);
}
