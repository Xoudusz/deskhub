// Games page: the Steam game being played (cover, session time, total
// playtime) and the last few games played, which start on the PC when tapped.
#include <stdio.h>
#include <string.h>

#include "esp_heap_caps.h"
#include "hub_server.h"
#include "icons.h"
#include "ui.h"
#include "ui_internal.h"

#define HDR_W 340
#define HDR_H 159
#define THUMB_W 128
#define THUMB_H 60
#define PORT_W 96
#define PORT_H 144
// The header shows smaller above the bottom dock, so the recent covers keep their size.
#define IMG_W (UI_COMPACT ? 218 : HDR_W)
#define IMG_H (UI_COMPACT ? 102 : HDR_H)
#define TOP_H (IMG_H + 32)

hub_steam_t g_steam;

// One cover image in PSRAM, tagged with the game it belongs to.
typedef struct {
    uint32_t appid;
    uint8_t *px;
    size_t cap;
    lv_image_dsc_t dsc;
} cover_t;

static cover_t s_hdr, s_thumb, s_port[HUB_STEAM_RECENT];
static uint32_t s_rx_tick;   // lv_tick when the last steam message arrived

static lv_obj_t *s_now_ph, *s_now_img, *s_now_name, *s_now_time, *s_now_total, *s_now_cap;
static lv_obj_t *s_idle, *s_idle_sub, *s_now_box;
static lv_obj_t *s_fr_cap, *s_fr_name[HUB_STEAM_FRIENDS], *s_fr_game[HUB_STEAM_FRIENDS];
static lv_obj_t *s_tile[HUB_STEAM_RECENT], *s_tile_img[HUB_STEAM_RECENT], *s_tile_ph[HUB_STEAM_RECENT];
static lv_obj_t *s_tile_name[HUB_STEAM_RECENT], *s_tile_hours[HUB_STEAM_RECENT];
static lv_obj_t *s_recent_empty;

// ---- placeholder for games without art ---------------------------------------
// A rounded tile in a muted colour picked from the game's name, with its initials.

static uint32_t name_color(const char *name)
{
    static const uint32_t pal[8] = {0x2F5D8A, 0x2E7D6B, 0x6B4E9B, 0x9A4A5A,
                                    0x8A6A2F, 0x4A6B2F, 0x34707F, 0x7A4A2E};
    uint32_t h = 2166136261u;   // FNV-1a over the lower-cased name
    for (const char *p = name; *p; p++) {
        char c = *p;
        if (c >= 'A' && c <= 'Z') c += 32;
        h = (h ^ (uint8_t)c) * 16777619u;
    }
    return pal[(h >> 8) % 8];
}

// First letter of each word, up to 3; a single word gives its first 2 letters.
static void name_initials(char *out, size_t n, const char *name)
{
    char first[4] = {0}, second = 0;
    int words = 0;
    bool start = true;
    for (const char *p = name; *p; p++) {
        char c = *p;
        bool alnum = (c >= '0' && c <= '9') || (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z');
        if (!alnum) {
            start = true;
            continue;
        }
        if (c >= 'a' && c <= 'z') c -= 32;
        if (start) {
            if (words < 3) first[words] = c;
            words++;
        } else if (words == 1 && !second) {
            second = c;
        }
        start = false;
    }
    if (words == 1 && second) snprintf(out, n, "%c%c", first[0], second);
    else if (words) snprintf(out, n, "%.3s", first);
    else snprintf(out, n, "?");
}

void games_placeholder_init(lv_obj_t *ph, const lv_font_t *font)
{
    lv_obj_set_style_radius(ph, 10, 0);
    lv_obj_set_style_bg_opa(ph, LV_OPA_COVER, 0);
    lv_obj_t *l = ui_label(ph, font, 0xFFFFFF, "");
    lv_obj_center(l);
}

void games_placeholder_set(lv_obj_t *ph, const char *name)
{
    bool has = name && *name;
    char ini[4] = "";
    if (has) name_initials(ini, sizeof(ini), name);
    lv_obj_set_style_bg_color(ph, lv_color_hex(has ? name_color(name) : COL_CARD_HI), 0);
    lv_label_set_text(lv_obj_get_child(ph, 0), ini);
}

void ui_fmt_playtime(char *buf, size_t n, int minutes)
{
    if (minutes >= 600) snprintf(buf, n, "%d h", minutes / 60);
    else if (minutes >= 60) snprintf(buf, n, "%.1f h", minutes / 60.0);
    else snprintf(buf, n, "%d min", minutes);
}

static void fmt_session(char *buf, size_t n, int s)
{
    if (s >= 3600) snprintf(buf, n, "Playing for %dh %02dm", s / 3600, (s / 60) % 60);
    else snprintf(buf, n, "Playing for %dm", s / 60);
}

static void fmt_ago(char *buf, size_t n, time_t t)
{
    long d = (long)(time(NULL) - t);
    if (t <= 0) buf[0] = 0;
    else if (d < 3600) snprintf(buf, n, "Just now");
    else if (d < 86400) snprintf(buf, n, "%ldh ago", d / 3600);
    else if (d < 2 * 86400) snprintf(buf, n, "Yesterday");
    else if (d < 60 * 86400) snprintf(buf, n, "%ld days ago", d / 86400);
    else snprintf(buf, n, "%ld months ago", d / (30 * 86400));
}

// A total only adds something once it is clearly more than this session
// (during a first session the two are the same).
bool games_total_worth_showing(int total_min, int session_s)
{
    return total_min > 0 && total_min * 60 >= session_s + 600;
}

int games_session_s(void)
{
    if (!g_steam.playing) return 0;
    return g_steam.session_s + (int)(lv_tick_elaps(s_rx_tick) / 1000);
}

void games_launch(uint32_t appid)
{
    char buf[64];
    snprintf(buf, sizeof(buf), "{\"type\":\"steam_launch\",\"appid\":%lu}", (unsigned long)appid);
    hub_server_send(buf);
}

static void tile_cb(lv_event_t *e)
{
    int i = (int)(intptr_t)lv_event_get_user_data(e);
    if (i < g_steam.recent_count) games_launch(g_steam.recent[i].appid);
}

static bool cover_set(cover_t *c, uint32_t appid, uint16_t w, uint16_t h, const uint8_t *px)
{
    size_t bytes = (size_t)w * h * 2;
    if (bytes > c->cap) {
        uint8_t *p = heap_caps_realloc(c->px, bytes, MALLOC_CAP_SPIRAM);
        if (!p) return false;
        c->px = p;
        c->cap = bytes;
    }
    lv_image_cache_drop(&c->dsc);
    memcpy(c->px, px, bytes);
    memset(&c->dsc, 0, sizeof(c->dsc));
    c->dsc.header.magic = LV_IMAGE_HEADER_MAGIC;
    c->dsc.header.cf = LV_COLOR_FORMAT_RGB565;
    c->dsc.header.w = w;
    c->dsc.header.h = h;
    c->dsc.header.stride = w * 2;
    c->dsc.data_size = bytes;
    c->dsc.data = c->px;
    c->appid = appid;
    return true;
}

const lv_image_dsc_t *games_thumb(uint32_t *appid)
{
    if (!s_thumb.appid) return NULL;
    if (appid) *appid = s_thumb.appid;
    return &s_thumb.dsc;
}

static const cover_t *portrait_for(uint32_t appid)
{
    for (int i = 0; i < HUB_STEAM_RECENT; i++)
        if (s_port[i].appid == appid) return &s_port[i];
    return NULL;
}

void ui_set_steam_art(uint32_t appid, int kind, uint16_t w, uint16_t h, const uint8_t *px)
{
    if (kind == 0) {
        cover_set(&s_hdr, appid, w, h, px);
    } else if (kind == 1) {
        cover_set(&s_thumb, appid, w, h, px);
        home_refresh_game();
    } else if (kind == 2) {
        // Reuse this game's slot, else one no recent game uses any more.
        cover_t *slot = (cover_t *)portrait_for(appid);
        for (int i = 0; !slot && i < HUB_STEAM_RECENT; i++) {
            bool used = false;
            for (int j = 0; j < g_steam.recent_count; j++) used |= g_steam.recent[j].appid == s_port[i].appid;
            if (!used || !s_port[i].appid) slot = &s_port[i];
        }
        if (slot) cover_set(slot, appid, w, h, px);
    }
    games_refresh();
}

void ui_set_steam(const hub_steam_t *s)
{
    g_steam = *s;
    s_rx_tick = lv_tick_get();
    games_refresh();
    home_refresh_game();
}

void games_page_create(lv_obj_t *page)
{
    // Now playing.
    lv_obj_t *top = ui_card(page);
    lv_obj_set_size(top, CONTENT_W, TOP_H);
    lv_obj_set_pos(top, 0, 0);

    s_now_box = lv_obj_create(top);
    lv_obj_remove_style_all(s_now_box);
    lv_obj_set_size(s_now_box, LV_PCT(100), LV_PCT(100));
    lv_obj_remove_flag(s_now_box, LV_OBJ_FLAG_SCROLLABLE);
    s_now_ph = lv_obj_create(s_now_box);   // shown while the game has no cover
    lv_obj_remove_style_all(s_now_ph);
    lv_obj_remove_flag(s_now_ph, LV_OBJ_FLAG_CLICKABLE);
    lv_obj_set_size(s_now_ph, IMG_W, IMG_H);
    lv_obj_set_pos(s_now_ph, 0, 0);
    games_placeholder_init(s_now_ph, g_font_48);
    lv_obj_set_style_radius(s_now_ph, 12, 0);
    s_now_img = lv_image_create(s_now_box);
    lv_obj_set_size(s_now_img, IMG_W, IMG_H);
    lv_image_set_inner_align(s_now_img, LV_IMAGE_ALIGN_STRETCH);
    lv_obj_set_style_radius(s_now_img, 12, 0);
    lv_obj_set_style_clip_corner(s_now_img, true, 0);
    lv_obj_set_style_bg_color(s_now_img, lv_color_hex(COL_CARD_HI), 0);
    lv_obj_set_style_bg_opa(s_now_img, LV_OPA_COVER, 0);
    lv_obj_set_pos(s_now_img, 0, 0);
    const int tx = IMG_W + 20, tw = CONTENT_W - 32 - tx;
    s_now_cap = ui_caption(s_now_box, "NOW PLAYING");
    lv_obj_set_style_text_color(s_now_cap, lv_color_hex(COL_GOOD), 0);
    lv_obj_set_pos(s_now_cap, tx, 4);
    s_now_name = ui_label(s_now_box, g_font_24, COL_TEXT, "");
    ui_marquee(s_now_name);
    lv_obj_set_size(s_now_name, tw, UI_COMPACT ? 30 : 62);
    lv_obj_set_pos(s_now_name, tx, UI_COMPACT ? 24 : 30);
    s_now_time = ui_label(s_now_box, g_font_20, COL_TEXT, "");
    lv_obj_set_pos(s_now_time, tx, UI_COMPACT ? 54 : IMG_H - 54);
    s_now_total = ui_label(s_now_box, g_font_16, COL_MUTED, "");
    lv_obj_set_pos(s_now_total, tx, UI_COMPACT ? 80 : IMG_H - 24);

    s_idle = lv_obj_create(top);
    lv_obj_remove_style_all(s_idle);
    lv_obj_set_size(s_idle, LV_PCT(100), LV_PCT(100));
    lv_obj_remove_flag(s_idle, LV_OBJ_FLAG_SCROLLABLE);
    lv_obj_t *ic = ui_label(s_idle, g_icon_80, COL_DIM, ICON_SPORTS_ESPORTS);
    lv_obj_align(ic, LV_ALIGN_LEFT_MID, 24, 0);
    lv_obj_t *t = ui_label(s_idle, g_font_24, COL_TEXT, "No game running");
    lv_obj_align(t, LV_ALIGN_LEFT_MID, 136, -22);
    s_idle_sub = ui_label(s_idle, g_font_16, COL_MUTED, "");
    lv_obj_set_width(s_idle_sub, 372 - 136 - 16);   // wraps clear of the friends list
    lv_obj_align(s_idle_sub, LV_ALIGN_LEFT_MID, 136, 22);

    // Friends in a game (only with a Steam Web API key), on the right.
    const int fx = 372, fw = CONTENT_W - 32 - fx;
    s_fr_cap = ui_caption(s_idle, "FRIENDS IN GAME");
    lv_obj_set_pos(s_fr_cap, fx, 0);
    for (int i = 0; i < HUB_STEAM_FRIENDS; i++) {
        s_fr_name[i] = ui_label(s_idle, g_font_16, COL_TEXT, "");
        ui_marquee(s_fr_name[i]);
        lv_obj_set_size(s_fr_name[i], fw * 2 / 5, 20);
        lv_obj_set_pos(s_fr_name[i], fx, 24 + i * 27);
        s_fr_game[i] = ui_label(s_idle, g_font_14, COL_GOOD, "");
        ui_marquee(s_fr_game[i]);
        lv_obj_set_size(s_fr_game[i], fw * 3 / 5 - 8, 18);
        lv_obj_set_pos(s_fr_game[i], fx + fw * 2 / 5 + 8, 26 + i * 27);
    }

    // Recent games: covers that start the game on the PC.
    lv_obj_t *bot = ui_card(page);
    lv_obj_set_size(bot, CONTENT_W, CONTENT_H - TOP_H - 12);
    lv_obj_set_pos(bot, 0, TOP_H + 12);
    lv_obj_t *cap = ui_caption(bot, "RECENT  " "•" "  TAP TO PLAY");
    lv_obj_set_pos(cap, 0, 0);
    const int step = (CONTENT_W - 32 - PORT_W) / (HUB_STEAM_RECENT - 1);
    for (int i = 0; i < HUB_STEAM_RECENT; i++) {
        lv_obj_t *b = lv_button_create(bot);
        lv_obj_remove_style_all(b);
        lv_obj_set_size(b, step - 8, PORT_H + 26);
        lv_obj_set_pos(b, i * step - (step - 8 - PORT_W) / 2, 24);
        lv_obj_set_style_transform_scale(b, 240, LV_STATE_PRESSED);
        lv_obj_set_style_transform_pivot_x(b, (step - 8) / 2, 0);
        lv_obj_set_style_transform_pivot_y(b, PORT_H / 2, 0);
        lv_obj_add_event_cb(b, tile_cb, LV_EVENT_CLICKED, (void *)(intptr_t)i);
        lv_obj_t *ph = lv_obj_create(b);   // placeholder until the cover arrives
        lv_obj_remove_style_all(ph);
        lv_obj_remove_flag(ph, LV_OBJ_FLAG_CLICKABLE);
        lv_obj_set_size(ph, PORT_W, PORT_H);
        lv_obj_align(ph, LV_ALIGN_TOP_MID, 0, 0);
        games_placeholder_init(ph, g_font_32);
        lv_obj_t *img = lv_image_create(b);
        lv_obj_remove_flag(img, LV_OBJ_FLAG_CLICKABLE);
        lv_obj_set_size(img, PORT_W, PORT_H);
        lv_obj_set_style_radius(img, 10, 0);
        lv_obj_set_style_clip_corner(img, true, 0);
        lv_obj_align(img, LV_ALIGN_TOP_MID, 0, 0);
        lv_obj_t *name = ui_label(b, g_font_14, COL_MUTED, "");
        ui_marquee(name);
        lv_obj_set_size(name, step - 8, 18);
        lv_obj_set_style_text_align(name, LV_TEXT_ALIGN_CENTER, 0);
        lv_obj_align(name, LV_ALIGN_BOTTOM_MID, 0, 0);
        // Total playtime, as a small badge on the cover.
        lv_obj_t *hrs = ui_label(b, g_font_14, COL_TEXT, "");
        lv_obj_remove_flag(hrs, LV_OBJ_FLAG_CLICKABLE);
        lv_obj_set_style_bg_color(hrs, lv_color_hex(0x000000), 0);
        lv_obj_set_style_bg_opa(hrs, LV_OPA_70, 0);
        lv_obj_set_style_radius(hrs, 8, 0);
        lv_obj_set_style_pad_hor(hrs, 6, 0);
        lv_obj_set_style_pad_ver(hrs, 2, 0);
        lv_obj_align_to(hrs, ph, LV_ALIGN_BOTTOM_RIGHT, -4, -4);
        s_tile_hours[i] = hrs;
        s_tile[i] = b;
        s_tile_img[i] = img;
        s_tile_ph[i] = ph;
        s_tile_name[i] = name;
    }
    s_recent_empty = ui_label(bot, g_font_16, COL_DIM, "");
    lv_obj_align(s_recent_empty, LV_ALIGN_CENTER, 0, 10);
    games_refresh();
}

void games_refresh(void)
{
    if (!s_now_box) return;
    char buf[48], tot[16];
    const hub_steam_t *s = &g_steam;
    if (s->playing) {
        lv_obj_remove_flag(s_now_box, LV_OBJ_FLAG_HIDDEN);
        lv_obj_add_flag(s_idle, LV_OBJ_FLAG_HIDDEN);
        lv_label_set_text(s_now_name, s->now.name);
        games_placeholder_set(s_now_ph, s->now.name);
        if (s_hdr.appid == s->now.appid) {
            lv_image_set_src(s_now_img, &s_hdr.dsc);
            lv_obj_remove_flag(s_now_img, LV_OBJ_FLAG_HIDDEN);
        } else {
            lv_image_set_src(s_now_img, NULL);
            lv_obj_add_flag(s_now_img, LV_OBJ_FLAG_HIDDEN);
        }
        ui_fmt_playtime(tot, sizeof(tot), s->now.total_min);
        buf[0] = 0;
        if (games_total_worth_showing(s->now.total_min, games_session_s())) snprintf(buf, sizeof(buf), "%s played", tot);
        if (s->friend_count) {
            char fr[32];
            snprintf(fr, sizeof(fr), "%s%d friend%s in game", buf[0] ? "  " "•" "  " : "",
                     s->friend_count, s->friend_count == 1 ? "" : "s");
            strlcat(buf, fr, sizeof(buf));
        }
        lv_label_set_text(s_now_total, buf);
        games_tick();
    } else {
        lv_obj_add_flag(s_now_box, LV_OBJ_FLAG_HIDDEN);
        lv_obj_remove_flag(s_idle, LV_OBJ_FLAG_HIDDEN);
        lv_label_set_text(s_idle_sub, !s->valid ? (g_pc_online ? "Waiting for the PC" : "PC offline")
                                      : !s->found ? "Steam isn't installed on the PC"
                                      : "Start a game, or pick one below");
    }
    const int rows = (IMG_H - 24) / 27;   // friend rows that fit in the card
    for (int i = 0; i < HUB_STEAM_FRIENDS; i++) {
        bool on = i < s->friend_count && i < rows;
        lv_label_set_text(s_fr_name[i], on ? s->friend_name[i] : "");
        lv_label_set_text(s_fr_game[i], on ? s->friend_game[i] : "");
    }
    if (s->friend_count) lv_obj_remove_flag(s_fr_cap, LV_OBJ_FLAG_HIDDEN);
    else lv_obj_add_flag(s_fr_cap, LV_OBJ_FLAG_HIDDEN);

    for (int i = 0; i < HUB_STEAM_RECENT; i++) {
        bool on = i < s->recent_count;
        if (!on) {
            lv_obj_add_flag(s_tile[i], LV_OBJ_FLAG_HIDDEN);
            continue;
        }
        lv_obj_remove_flag(s_tile[i], LV_OBJ_FLAG_HIDDEN);
        lv_label_set_text(s_tile_name[i], s->recent[i].name);
        games_placeholder_set(s_tile_ph[i], s->recent[i].name);
        if (s->recent[i].total_min > 0) {
            ui_fmt_playtime(tot, sizeof(tot), s->recent[i].total_min);
            lv_label_set_text(s_tile_hours[i], tot);
            lv_obj_remove_flag(s_tile_hours[i], LV_OBJ_FLAG_HIDDEN);
            lv_obj_align_to(s_tile_hours[i], s_tile_ph[i], LV_ALIGN_BOTTOM_RIGHT, -4, -4);
        } else {
            lv_obj_add_flag(s_tile_hours[i], LV_OBJ_FLAG_HIDDEN);
        }
        const cover_t *c = portrait_for(s->recent[i].appid);
        if (c) {
            lv_image_set_src(s_tile_img[i], &c->dsc);
            lv_obj_remove_flag(s_tile_img[i], LV_OBJ_FLAG_HIDDEN);
        } else {
            lv_obj_add_flag(s_tile_img[i], LV_OBJ_FLAG_HIDDEN);
        }
    }
    lv_label_set_text(s_recent_empty, s->recent_count ? "" : s->found ? "No recent games" : "");
}

void games_tick(void)
{
    if (g_steam.playing) home_refresh_game();   // its session time
    if (!s_now_time || !g_steam.playing) return;
    char buf[40];
    fmt_session(buf, sizeof(buf), games_session_s());
    if (strcmp(lv_label_get_text(s_now_time), buf) != 0) lv_label_set_text(s_now_time, buf);
}

// Used by the Home widget's "last played" line.
void games_fmt_ago(char *buf, size_t n, time_t t)
{
    fmt_ago(buf, n, t);
}
