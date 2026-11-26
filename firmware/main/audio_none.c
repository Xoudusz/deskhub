// Boards without a speaker codec (the S3 4.3B, the simulator): sounds are no-ops.
#include "audio.h"

void audio_init(void) {}
bool audio_available(void) { return false; }
void audio_set_volume(int volume) { (void)volume; }
void audio_play(sound_t s) { (void)s; }
