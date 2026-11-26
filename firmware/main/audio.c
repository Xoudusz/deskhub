// Speaker on the ESP32-P4-WIFI6-Touch-LCD-4.3: ES8311 codec over I2C (the touch
// bus) and I2S, amplifier enabled on GPIO53. Pins and codec settings follow
// Waveshare's BSP (esp32_p4_wifi6_touch_lcd_4_3 1.0.1). Sounds are synthesised
// here, so there are no sound files; the codec is opened per sound, so the
// amplifier is off (no hiss) while nothing plays.
#include <math.h>
#include <string.h>

#include "audio.h"
#include "board.h"
#include "driver/i2s_std.h"
#include "esp_codec_dev.h"
#include "esp_codec_dev_defaults.h"
#include "esp_log.h"
#include "freertos/FreeRTOS.h"
#include "freertos/queue.h"
#include "freertos/task.h"

static const char *TAG = "audio";

#define PIN_MCLK  13
#define PIN_BCLK  12
#define PIN_WS    10
#define PIN_DOUT  9
#define PIN_PA    53
#define RATE      16000
#define CHUNK     256          // frames per write (16 ms)

static esp_codec_dev_handle_t s_dev;
static QueueHandle_t s_queue;
static volatile int s_volume = 60;

// One note: frequency, length, how fast it fades (time constant), overtone mix.
typedef struct {
    float hz;
    int ms;
    float tau_ms;
    float overtone;   // share of the 2nd and 3rd harmonics (bell-like when > 0)
} note_t;

typedef struct {
    const note_t *notes;
    int count;
    int gap_ms;       // silence between notes
} tune_t;

static const note_t CHIME[] = {{880.0f, 170, 120, 0.15f}, {1318.5f, 420, 220, 0.15f}};
static const note_t BELL[] = {{659.3f, 1100, 380, 0.5f}};
static const note_t BEEP[] = {{1000.0f, 90, 1000, 0.25f}, {1000.0f, 90, 1000, 0.25f}};
static const note_t SOFT[] = {{523.3f, 260, 200, 0.0f}, {659.3f, 380, 260, 0.0f}};

static const tune_t TUNES[SOUND_COUNT] = {
    [SOUND_CHIME] = {CHIME, 2, 0},
    [SOUND_BELL] = {BELL, 1, 0},
    [SOUND_BEEP] = {BEEP, 2, 70},
    [SOUND_SOFT] = {SOFT, 2, 20},
};

// Writes `frames` stereo frames from `buf`. Returns false when a newer sound is
// waiting, so the current one stops.
static bool write_frames(int16_t *buf, int frames)
{
    esp_codec_dev_write(s_dev, buf, frames * 2 * sizeof(int16_t));
    return uxQueueMessagesWaiting(s_queue) == 0;
}

static bool play_note(const note_t *n, int16_t *buf)
{
    const int total = RATE * n->ms / 1000;
    const int attack = RATE * 5 / 1000;
    const int release = RATE * 8 / 1000;
    const float w = 2.0f * (float)M_PI * n->hz / RATE;
    const float decay = expf(-1000.0f / (n->tau_ms * RATE));
    float env = 1.0f;
    for (int i = 0; i < total;) {
        int frames = total - i < CHUNK ? total - i : CHUNK;
        for (int k = 0; k < frames; k++, i++) {
            float a = env;
            if (i < attack) a *= (float)i / attack;
            if (total - i < release) a *= (float)(total - i) / release;
            float t = w * i;
            float s = sinf(t) + n->overtone * (0.6f * sinf(2.0f * t) + 0.4f * sinf(3.0f * t));
            int16_t v = (int16_t)(s / (1.0f + n->overtone) * a * 0.7f * 32767.0f);
            buf[2 * k] = v;
            buf[2 * k + 1] = v;
            env *= decay;
        }
        if (!write_frames(buf, frames)) return false;
    }
    return true;
}

static bool silence(int ms, int16_t *buf)
{
    memset(buf, 0, CHUNK * 2 * sizeof(int16_t));
    for (int left = RATE * ms / 1000; left > 0; left -= CHUNK) {
        if (!write_frames(buf, left < CHUNK ? left : CHUNK)) return false;
    }
    return true;
}

static void play(sound_t s)
{
    if (s <= SOUND_OFF || s >= SOUND_COUNT || !TUNES[s].notes) return;
    static int16_t buf[CHUNK * 2];
    esp_codec_dev_sample_info_t fs = {
        .bits_per_sample = 16,
        .channel = 2,
        .channel_mask = 0x03,
        .sample_rate = RATE,
        .mclk_multiple = 256,
    };
    if (esp_codec_dev_open(s_dev, &fs) != ESP_CODEC_DEV_OK) {
        ESP_LOGW(TAG, "codec open failed");
        return;
    }
    esp_codec_dev_set_out_vol(s_dev, s_volume);
    const tune_t *t = &TUNES[s];
    bool go = silence(20, buf);   // let the amplifier settle
    for (int i = 0; go && i < t->count; i++) {
        go = play_note(&t->notes[i], buf);
        if (go && t->gap_ms && i + 1 < t->count) go = silence(t->gap_ms, buf);
    }
    if (go) silence(40, buf);     // flush the DMA buffers before the amp goes off
    esp_codec_dev_close(s_dev);
}

static void audio_task(void *arg)
{
    (void)arg;
    sound_t s;
    for (;;) {
        if (xQueueReceive(s_queue, &s, portMAX_DELAY) == pdTRUE) play(s);
    }
}

void audio_init(void)
{
    i2s_chan_handle_t tx = NULL;
    i2s_chan_config_t chan = I2S_CHANNEL_DEFAULT_CONFIG(I2S_NUM_0, I2S_ROLE_MASTER);
    chan.auto_clear = true;
    if (i2s_new_channel(&chan, &tx, NULL) != ESP_OK) {
        ESP_LOGE(TAG, "i2s channel");
        return;
    }
    i2s_std_config_t std = {
        .clk_cfg = I2S_STD_CLK_DEFAULT_CONFIG(RATE),
        .slot_cfg = I2S_STD_PHILIPS_SLOT_DEFAULT_CONFIG(I2S_DATA_BIT_WIDTH_16BIT, I2S_SLOT_MODE_STEREO),
        .gpio_cfg = {
            .mclk = PIN_MCLK,
            .bclk = PIN_BCLK,
            .ws = PIN_WS,
            .dout = PIN_DOUT,
            .din = I2S_GPIO_UNUSED,
        },
    };
    std.clk_cfg.mclk_multiple = I2S_MCLK_MULTIPLE_256;
    if (i2s_channel_init_std_mode(tx, &std) != ESP_OK || i2s_channel_enable(tx) != ESP_OK) {
        ESP_LOGE(TAG, "i2s init");
        return;
    }
    audio_codec_i2s_cfg_t i2s_cfg = {.port = I2S_NUM_0, .tx_handle = tx};
    const audio_codec_data_if_t *data_if = audio_codec_new_i2s_data(&i2s_cfg);
    audio_codec_i2c_cfg_t i2c_cfg = {
        .port = 1,   // the touch bus (board_p4.c), already set up by board_init()
        .addr = ES8311_CODEC_DEFAULT_ADDR,
        .bus_handle = board_i2c_bus(),
    };
    const audio_codec_ctrl_if_t *ctrl_if = audio_codec_new_i2c_ctrl(&i2c_cfg);
    const audio_codec_gpio_if_t *gpio_if = audio_codec_new_gpio();
    if (!data_if || !ctrl_if || !gpio_if) {
        ESP_LOGE(TAG, "codec interfaces");
        return;
    }
    es8311_codec_cfg_t es = {
        .ctrl_if = ctrl_if,
        .gpio_if = gpio_if,
        .codec_mode = ESP_CODEC_DEV_WORK_MODE_DAC,
        .pa_pin = PIN_PA,
        .pa_reverted = false,
        .master_mode = false,
        .use_mclk = true,
        .hw_gain = {.pa_voltage = 5.0f, .codec_dac_voltage = 3.3f},
    };
    const audio_codec_if_t *codec = es8311_codec_new(&es);
    if (!codec) {
        ESP_LOGE(TAG, "ES8311 not found");
        return;
    }
    esp_codec_dev_cfg_t dev = {
        .dev_type = ESP_CODEC_DEV_TYPE_OUT,
        .codec_if = codec,
        .data_if = data_if,
    };
    s_dev = esp_codec_dev_new(&dev);
    if (!s_dev) return;
    s_queue = xQueueCreate(2, sizeof(sound_t));
    if (!s_queue || xTaskCreate(audio_task, "audio", 4096, NULL, 3, NULL) != pdPASS) {
        s_dev = NULL;
        return;
    }
    ESP_LOGI(TAG, "speaker ready");
}

bool audio_available(void)
{
    return s_dev != NULL;
}

void audio_set_volume(int volume)
{
    s_volume = volume < 0 ? 0 : volume > 100 ? 100 : volume;
}

void audio_play(sound_t s)
{
    if (!s_dev || s <= SOUND_OFF || s >= SOUND_COUNT) return;
    xQueueReset(s_queue);   // newest wins
    xQueueSend(s_queue, &s, 0);
}
