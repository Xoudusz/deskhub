// Sounds through the board's speaker.
//
// audio.c drives the ESP32-P4 4.3 board's ES8311 codec and amplifier; boards
// without one (the S3 4.3B, the simulator) build audio_none.c, where playing is
// a no-op. sounds.c decides which sound an event plays (Settings > Sound in the
// desktop app, sent with the "layout" message and kept in NVS).
#pragma once

#include <stdbool.h>

typedef enum { SOUND_OFF, SOUND_CHIME, SOUND_BELL, SOUND_BEEP, SOUND_SOFT, SOUND_COUNT } sound_t;

typedef enum { SOUND_EV_ALERT, SOUND_EV_TIMER, SOUND_EV_REMINDER, SOUND_EV_COUNT } sound_event_t;

// ---- audio.c / audio_none.c: the hardware ----
void audio_init(void);
bool audio_available(void);
// Volume 0-100 for the next sounds.
void audio_set_volume(int volume);
// Returns at once; a newer sound cuts off one still playing.
void audio_play(sound_t s);

// ---- sounds.c: settings and events ----
sound_t sound_from_name(const char *name);
// The "sound" object of the layout message: {mute, volume, alert, timer, reminder, remind_min}.
struct cJSON;
void sounds_set(const struct cJSON *cfg);
// Plays the sound chosen for `ev`, unless muted or set to off.
void sounds_event(sound_event_t ev);
// Settings > Sound > Test: plays `name` at the set volume, even when muted.
void sounds_test(const char *name);
// Once a second from the UI tick: calendar reminders.
void sounds_tick(void);
