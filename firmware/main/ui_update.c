// The firmware update prompt: "Install a firmware update?" with Deny / Install,
// then a progress bar while it installs. Shown above everything else.
#include <stdio.h>
#include "icons.h"
#include "ota.h"
#include "ui.h"
#include "ui_internal.h"

static lv_obj_t *s_root, *s_title, *s_sub, *s_buttons, *s_bar;

static void answer_cb(lv_event_t *e)
{
    bool allow = (bool)(intptr_t)lv_event_get_user_data(e);
    ota_answer(allow);
    lv_obj_add_flag(s_buttons, LV_OBJ_FLAG_HIDDEN);
    lv_label_set_text(s_sub, allow ? "Starting..." : "Not installed");
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
    // A dark scrim over the page, and a card in the middle.
    s_root = lv_obj_create(lv_layer_top());
    lv_obj_remove_style_all(s_root);
    lv_obj_set_size(s_root, 800, 480);
    lv_obj_set_style_bg_color(s_root, lv_color_hex(COL_BG), 0);
    lv_obj_set_style_bg_opa(s_root, LV_OPA_COVER, 0);
    lv_obj_add_flag(s_root, LV_OBJ_FLAG_CLICKABLE);   // nothing behind it reacts
    lv_obj_remove_flag(s_root, LV_OBJ_FLAG_SCROLLABLE);

    lv_obj_t *card = ui_card(s_root);
    lv_obj_set_size(card, 560, 280);
    lv_obj_center(card);
    lv_obj_set_style_pad_all(card, 28, 0);
    lv_obj_set_flex_flow(card, LV_FLEX_FLOW_COLUMN);
    lv_obj_set_flex_align(card, LV_FLEX_ALIGN_START, LV_FLEX_ALIGN_CENTER, LV_FLEX_ALIGN_CENTER);
    lv_obj_set_style_pad_row(card, 10, 0);

    ui_label(card, g_icon_48, COL_ACCENT, ICON_SYSTEM_UPDATE);
    s_title = ui_label(card, g_font_24, COL_TEXT, "Install a firmware update?");
    s_sub = ui_label(card, g_font_16, COL_MUTED, "Your PC sent a new version of Desk Hub.");
    lv_obj_set_width(s_sub, 500);
    lv_obj_set_style_text_align(s_sub, LV_TEXT_ALIGN_CENTER, 0);

    s_bar = ui_bar(card, COL_ACCENT);
    lv_obj_set_size(s_bar, 440, 10);
    lv_obj_set_style_margin_top(s_bar, 18, 0);

    s_buttons = lv_obj_create(card);
    lv_obj_remove_style_all(s_buttons);
    lv_obj_set_size(s_buttons, LV_SIZE_CONTENT, LV_SIZE_CONTENT);
    lv_obj_set_flex_flow(s_buttons, LV_FLEX_FLOW_ROW);
    lv_obj_set_style_pad_column(s_buttons, 20, 0);
    lv_obj_set_style_margin_top(s_buttons, 14, 0);
    button(s_buttons, "Deny", false);
    button(s_buttons, "Install", true);
}

void ui_update_show(void)
{
    if (!s_root) build();
    if (ui_saver_shown()) ui_saver_show(false);
    lv_label_set_text(s_title, "Install a firmware update?");
    lv_label_set_text(s_sub, "Your PC sent a new version of Desk Hub.");
    lv_obj_remove_flag(s_buttons, LV_OBJ_FLAG_HIDDEN);
    lv_obj_add_flag(s_bar, LV_OBJ_FLAG_HIDDEN);
    lv_obj_remove_flag(s_root, LV_OBJ_FLAG_HIDDEN);
    lv_obj_move_foreground(s_root);
}

void ui_update_progress(int pct)
{
    if (!s_root) return;
    char buf[48];
    snprintf(buf, sizeof(buf), "Installing... %d%%", pct);
    lv_label_set_text(s_title, "Updating Desk Hub");
    lv_label_set_text(s_sub, buf);
    lv_obj_add_flag(s_buttons, LV_OBJ_FLAG_HIDDEN);
    lv_obj_remove_flag(s_bar, LV_OBJ_FLAG_HIDDEN);
    lv_bar_set_value(s_bar, pct, LV_ANIM_OFF);
}

void ui_update_message(const char *text)
{
    if (!s_root) return;
    lv_label_set_text(s_sub, text);
    lv_obj_add_flag(s_buttons, LV_OBJ_FLAG_HIDDEN);
}

// A release image on a board without Wi-Fi settings: nothing works until a local build
// (with sdkconfig.local) has run once and stored them.
void ui_setup_needed_show(void)
{
    if (!s_root) build();
    lv_label_set_text(s_title, "Desk Hub needs setup");
    lv_label_set_text(s_sub, "This board has no Wi-Fi settings yet. Flash a local build once "
                             "(firmware/build.sh with sdkconfig.local); later updates come from the app.");
    lv_obj_add_flag(s_buttons, LV_OBJ_FLAG_HIDDEN);
    lv_obj_add_flag(s_bar, LV_OBJ_FLAG_HIDDEN);
    lv_obj_remove_flag(s_root, LV_OBJ_FLAG_HIDDEN);
    lv_obj_move_foreground(s_root);
}

void ui_update_hide(void)
{
    if (s_root) lv_obj_add_flag(s_root, LV_OBJ_FLAG_HIDDEN);
}
