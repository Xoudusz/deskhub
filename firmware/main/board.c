#include "board.h"

#include <string.h>
#include "driver/gpio.h"
#include "driver/i2c_master.h"
#include "esp_check.h"
#include "esp_lcd_panel_ops.h"
#include "esp_lcd_panel_rgb.h"
#include "esp_log.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"

static const char *TAG = "board";

#define PIN_I2C_SDA   8
#define PIN_I2C_SCL   9
#define PIN_TOUCH_INT 4

// The CH422G is driven by writing single bytes to fixed "addresses":
// 0x24 sets the mode register, 0x38 sets the EXIO0-7 output levels.
#define CH422G_MODE   0x24
#define CH422G_OUT    0x38
#define EXIO_TP_RST   (1u << 1)
#define EXIO_BL       (1u << 2)
#define EXIO_LCD_RST  (1u << 3)

#define GT911_ADDR    0x5D
#define GT911_ADDR_ALT 0x14
#define GT911_STATUS  0x814E
#define GT911_POINT1  0x814F

static i2c_master_bus_handle_t s_bus;
static i2c_master_dev_handle_t s_exp_mode, s_exp_out, s_touch;
static esp_lcd_panel_handle_t s_panel;
static uint8_t s_exio;
static bool s_touch_down;
static uint16_t s_tx, s_ty;

static esp_err_t exio_set(uint8_t value)
{
    esp_err_t err = i2c_master_transmit(s_exp_out, &value, 1, 100);
    if (err == ESP_OK) s_exio = value;
    return err;
}

static esp_err_t add_dev(uint8_t addr, i2c_master_dev_handle_t *out)
{
    i2c_device_config_t cfg = {
        .dev_addr_length = I2C_ADDR_BIT_LEN_7,
        .device_address = addr,
        .scl_speed_hz = 400000,
    };
    return i2c_master_bus_add_device(s_bus, &cfg, out);
}

static esp_err_t io_init(void)
{
    i2c_master_bus_config_t bus = {
        .i2c_port = I2C_NUM_0,
        .sda_io_num = PIN_I2C_SDA,
        .scl_io_num = PIN_I2C_SCL,
        .clk_source = I2C_CLK_SRC_DEFAULT,
        .glitch_ignore_cnt = 7,
        .flags.enable_internal_pullup = true,
    };
    ESP_RETURN_ON_ERROR(i2c_new_master_bus(&bus, &s_bus), TAG, "i2c bus");
    ESP_RETURN_ON_ERROR(add_dev(CH422G_MODE, &s_exp_mode), TAG, "ch422g mode");
    ESP_RETURN_ON_ERROR(add_dev(CH422G_OUT, &s_exp_out), TAG, "ch422g out");

    uint8_t mode = 0x01; // push-pull outputs on EXIO0-7
    ESP_RETURN_ON_ERROR(i2c_master_transmit(s_exp_mode, &mode, 1, 100), TAG, "ch422g mode write");

    // GT911 latches its I2C address from INT during reset: INT low -> 0x5D.
    gpio_config_t intcfg = {.pin_bit_mask = 1ULL << PIN_TOUCH_INT, .mode = GPIO_MODE_OUTPUT};
    ESP_RETURN_ON_ERROR(gpio_config(&intcfg), TAG, "touch int");
    gpio_set_level(PIN_TOUCH_INT, 0);
    ESP_RETURN_ON_ERROR(exio_set(EXIO_LCD_RST), TAG, "reset low");
    vTaskDelay(pdMS_TO_TICKS(20));
    ESP_RETURN_ON_ERROR(exio_set(EXIO_LCD_RST | EXIO_TP_RST), TAG, "reset high");
    vTaskDelay(pdMS_TO_TICKS(100));
    gpio_reset_pin(PIN_TOUCH_INT);
    gpio_set_direction(PIN_TOUCH_INT, GPIO_MODE_INPUT);
    vTaskDelay(pdMS_TO_TICKS(50));

    // If the address strap did not take, the GT911 answers at 0x14 instead.
    uint8_t addr = GT911_ADDR;
    if (i2c_master_probe(s_bus, GT911_ADDR, 50) != ESP_OK &&
        i2c_master_probe(s_bus, GT911_ADDR_ALT, 50) == ESP_OK) {
        addr = GT911_ADDR_ALT;
    }
    ESP_LOGI(TAG, "GT911 at 0x%02x", addr);
    return add_dev(addr, &s_touch);
}

static esp_err_t panel_init(void)
{
    esp_lcd_rgb_panel_config_t cfg = {
        .clk_src = LCD_CLK_SRC_DEFAULT,
        .timings = {
            .pclk_hz = 16 * 1000 * 1000,
            .h_res = BOARD_WIDTH,
            .v_res = BOARD_HEIGHT,
            .hsync_pulse_width = 4,
            .hsync_back_porch = 8,
            .hsync_front_porch = 8,
            .vsync_pulse_width = 4,
            .vsync_back_porch = 8,
            .vsync_front_porch = 8,
            .flags.pclk_active_neg = 1,
        },
        .data_width = 16,
        .bits_per_pixel = 16,
        .num_fbs = 1,
        // Bounce buffers keep the panel fed from internal RAM while Wi-Fi
        // and flash access compete for PSRAM bandwidth.
        .bounce_buffer_size_px = BOARD_WIDTH * 10,
        .dma_burst_size = 64,
        .hsync_gpio_num = 46,
        .vsync_gpio_num = 3,
        .de_gpio_num = 5,
        .pclk_gpio_num = 7,
        .disp_gpio_num = -1,
        // B0-B4, G0-G5, R0-R4
        .data_gpio_nums = {14, 38, 18, 17, 10, 39, 0, 45, 48, 47, 21, 1, 2, 42, 41, 40},
        .flags.fb_in_psram = 1,
    };
    ESP_RETURN_ON_ERROR(esp_lcd_new_rgb_panel(&cfg, &s_panel), TAG, "rgb panel");
    ESP_RETURN_ON_ERROR(esp_lcd_panel_reset(s_panel), TAG, "panel reset");
    return esp_lcd_panel_init(s_panel);
}

esp_err_t board_init(void)
{
    ESP_RETURN_ON_ERROR(io_init(), TAG, "io");
    ESP_RETURN_ON_ERROR(panel_init(), TAG, "panel");
    board_backlight(true);
    ESP_LOGI(TAG, "board ready");
    return ESP_OK;
}

esp_lcd_panel_handle_t board_panel(void)
{
    return s_panel;
}

void board_backlight(bool on)
{
    uint8_t v = on ? (s_exio | EXIO_BL) : (s_exio & ~EXIO_BL);
    if (exio_set(v) != ESP_OK) {
        ESP_LOGW(TAG, "backlight write failed");
    }
}

void board_backlight_set(uint8_t pct)
{
    // The 4.3B switches its backlight through the I/O expander: on or off only.
    (void)pct;
}

bool board_has_dimming(void)
{
    return false;
}

bool board_snapshot(uint16_t *dst)
{
    void *fb = NULL;
    if (esp_lcd_rgb_panel_get_frame_buffer(s_panel, 1, &fb) != ESP_OK || !fb) return false;
    memcpy(dst, fb, (size_t)BOARD_WIDTH * BOARD_HEIGHT * 2);
    return true;
}

static esp_err_t gt911_read(uint16_t reg, uint8_t *buf, size_t len)
{
    uint8_t r[2] = {reg >> 8, reg & 0xff};
    return i2c_master_transmit_receive(s_touch, r, 2, buf, len, 50);
}

bool board_touch_read(uint16_t *x, uint16_t *y)
{
    uint8_t status;
    *x = s_tx;
    *y = s_ty;
    if (gt911_read(GT911_STATUS, &status, 1) != ESP_OK) {
        return false;
    }
    if (status & 0x80) {
        // Fresh data. Low nibble is the number of touch points.
        s_touch_down = (status & 0x0f) > 0;
        if (s_touch_down) {
            uint8_t p[8];
            if (gt911_read(GT911_POINT1, p, sizeof(p)) == ESP_OK) {
                s_tx = p[1] | (p[2] << 8);
                s_ty = p[3] | (p[4] << 8);
                if (s_tx >= BOARD_WIDTH) s_tx = BOARD_WIDTH - 1;
                if (s_ty >= BOARD_HEIGHT) s_ty = BOARD_HEIGHT - 1;
            }
        }
        uint8_t clear[3] = {GT911_STATUS >> 8, GT911_STATUS & 0xff, 0};
        i2c_master_transmit(s_touch, clear, sizeof(clear), 50);
    }
    *x = s_tx;
    *y = s_ty;
    return s_touch_down;
}
