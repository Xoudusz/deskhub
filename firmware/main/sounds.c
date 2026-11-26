// Which sound each board event plays, from Settings > Sound in the desktop app
// (the "sound" object of the layout message), kept in NVS ("hub"/"snd") so the
// board knows it before the app connects.
#include <string.h>
#include <time.h>

#include "audio.h"
#include "cJSON.h"
#include "nvs.h"
#include "ui_internal.h"

typedef struct {
    uint8_t mute;
    uint8_t volume;                 // 0-100
    uint8_t sound[SOUND_EV_COUNT];  // sound_t per event
    uint8_t remind_min;             // calendar reminder: minutes before the start
} sound_cfg_t;

static sound_cfg_t s_cfg = {
    .mute = 0,
    .volume = 60,
    .sound = {[SOUND_EV_ALERT] = SOUND_CHIME, [SOUND_EV_TIMER] = SOUND_BELL, [SOUND_EV_REMINDER] = SOUND_CHIME},
    .remind_min = 5,
};
static bool s_loaded;

static const char *const NAMES[SOUND_COUNT] = {"off", "chime", "bell", "beep", "soft"};

sound_t sound_from_name(const char *name)
{
    for (int i = 0; name && i < SOUND_COUNT; i++) {
        if (strcmp(name, NAMES[i]) == 0) return (sound_t)i;
    }
    return SOUND_OFF;
}

static void load(void)
{
    if (s_loaded) return;
    s_loaded = true;
    nvs_handle_t h;
    if (nvs_open("hub", NVS_READONLY, &h) == ESP_OK) {
        sound_cfg_t c;
        size_t n = sizeof(c);
        if (nvs_get_blob(h, "snd", &c, &n) == ESP_OK && n == sizeof(c)) s_cfg = c;
        nvs_close(h);
    }
    audio_set_volume(s_cfg.volume);
}

static int clamp(int v, int lo, int hi)
{
    return v < lo ? lo : v > hi ? hi : v;
}

void sounds_set(const cJSON *cfg)
{
    load();
    sound_cfg_t c = s_cfg;
    const cJSON *v;
    if (cJSON_IsBool(v = cJSON_GetObjectItemCaseSensitive(cfg, "mute"))) c.mute = cJSON_IsTrue(v);
    if (cJSON_IsNumber(v = cJSON_GetObjectItemCaseSensitive(cfg, "volume"))) c.volume = clamp(v->valueint, 0, 100);
    if (cJSON_IsNumber(v = cJSON_GetObjectItemCaseSensitive(cfg, "remind_min"))) c.remind_min = clamp(v->valueint, 0, 120);
    static const char *const KEYS[SOUND_EV_COUNT] = {"alert", "timer", "reminder"};
    for (int i = 0; i < SOUND_EV_COUNT; i++) {
        if (cJSON_IsString(v = cJSON_GetObjectItemCaseSensitive(cfg, KEYS[i]))) c.sound[i] = sound_from_name(v->valuestring);
    }
    if (memcmp(&c, &s_cfg, sizeof(c)) == 0) return;
    s_cfg = c;
    audio_set_volume(c.volume);
    nvs_handle_t h;
    if (nvs_open("hub", NVS_READWRITE, &h) == ESP_OK) {
        nvs_set_blob(h, "snd", &c, sizeof(c));
        nvs_commit(h);
        nvs_close(h);
    }
}

void sounds_event(sound_event_t ev)
{
    load();
    if (s_cfg.mute || ev < 0 || ev >= SOUND_EV_COUNT) return;
    audio_play((sound_t)s_cfg.sound[ev]);
}

void sounds_test(const char *name)
{
    load();
    audio_play(sound_from_name(name));
}

// Calendar reminders: each event chimes once, remind_min minutes before it
// starts (0 = at the start). Remembered by start time and title.
#define REMEMBER 16
static uint32_t s_done[REMEMBER];
static int s_done_next;

static uint32_t event_key(const hub_event_t *e)
{
    uint32_t h = 2166136261u ^ (uint32_t)e->start;
    for (const char *p = e->title; *p; p++) h = (h ^ (uint8_t)*p) * 16777619u;
    return h ? h : 1;
}

void sounds_tick(void)
{
    static int count;
    if (++count % 15) return;   // every 15 s is plenty
    load();
    time_t now = time(NULL);
    if (now < 1700000000) return;   // clock not set yet
    long lead = s_cfg.remind_min * 60L;
    for (int i = 0; i < g_event_count; i++) {
        const hub_event_t *e = &g_events[i];
        long until = (long)(e->start - now);
        if (e->all_day || until > lead || until <= -60) continue;
        uint32_t key = event_key(e);
        bool done = false;
        for (int k = 0; k < REMEMBER; k++) done |= s_done[k] == key;
        if (done) continue;
        s_done[s_done_next] = key;
        s_done_next = (s_done_next + 1) % REMEMBER;
        sounds_event(SOUND_EV_REMINDER);
        return;   // one chime per check
    }
}
