// Waveshare ESP32-P4-WIFI6-Touch-LCD-4.3: 480x800 ST7701 panel on 2-lane MIPI-DSI,
// GT911 touch on I2C, PWM backlight. The panel is portrait; display.c turns the
// 800x480 UI onto it. Pins and panel settings follow Waveshare's board package
// (waveshare/esp32_p4_wifi6_touch_lcd_4_3).
#include "board.h"

#include <string.h>
#include "driver/i2c_master.h"
#include "driver/ledc.h"
#include "esp_check.h"
#include "esp_ldo_regulator.h"
#include "esp_lcd_mipi_dsi.h"
#include "esp_lcd_panel_ops.h"
#include "esp_lcd_st7701.h"
#include "esp_log.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "sdkconfig.h"

static const char *TAG = "board";

#define PIN_I2C_SDA   7
#define PIN_I2C_SCL   8
#define PIN_LCD_RST   27
#define PIN_BACKLIGHT 26

// VDD_MIPI_DPHY comes from the chip's own LDO channel 3.
#define DSI_PHY_LDO_CHAN 3
#define DSI_PHY_LDO_MV   2500
#define DSI_LANES        2
#define DSI_LANE_MBPS    500
#define DPI_CLOCK_MHZ    30

#define BL_TIMER   LEDC_TIMER_1
#define BL_CHANNEL LEDC_CHANNEL_1
#define BL_MAX     1023   // 10-bit duty

#define GT911_ADDR     0x5D
#define GT911_ADDR_ALT 0x14
#define GT911_STATUS   0x814E
#define GT911_POINT1   0x814F

static i2c_master_bus_handle_t s_bus;
static i2c_master_dev_handle_t s_touch;
static esp_ldo_channel_handle_t s_phy_ldo;
static esp_lcd_dsi_bus_handle_t s_dsi;
static esp_lcd_panel_io_handle_t s_io;
static esp_lcd_panel_handle_t s_panel;
static uint8_t s_level = 100;   // backlight percent while on
static bool s_on;
static bool s_touch_down;
static uint16_t s_tx, s_ty;

// Panel init sequence from Waveshare's board package.
static const st7701_lcd_init_cmd_t s_init_cmds[] = {
    {0xFF, (uint8_t[]){0x77, 0x01, 0x00, 0x00, 0x13}, 5, 0},
    {0xEF, (uint8_t[]){0x08}, 1, 0},
    {0xFF, (uint8_t[]){0x77, 0x01, 0x00, 0x00, 0x10}, 5, 0},
    {0xC0, (uint8_t[]){0x63, 0x00}, 2, 0},
    {0xC1, (uint8_t[]){0x0D, 0x02}, 2, 0},
    {0xC2, (uint8_t[]){0x17, 0x08}, 2, 0},
    {0xCC, (uint8_t[]){0x10}, 1, 0},
    {0xB0, (uint8_t[]){0x40, 0xC9, 0x94, 0x0E, 0x10, 0x05, 0x0B, 0x09, 0x08, 0x26, 0x04, 0x52, 0x10, 0x69, 0x6B, 0x69}, 16, 0},
    {0xB1, (uint8_t[]){0x40, 0xD2, 0x98, 0x0C, 0x92, 0x07, 0x09, 0x08, 0x07, 0x25, 0x02, 0x0E, 0x0C, 0x6E, 0x78, 0x55}, 16, 0},
    {0xFF, (uint8_t[]){0x77, 0x01, 0x00, 0x00, 0x11}, 5, 0},
    {0xB0, (uint8_t[]){0x5D}, 1, 0},
    {0xB1, (uint8_t[]){0x4E}, 1, 0},
    {0xB2, (uint8_t[]){0x87}, 1, 0},
    {0xB3, (uint8_t[]){0x80}, 1, 0},
    {0xB5, (uint8_t[]){0x4E}, 1, 0},
    {0xB7, (uint8_t[]){0x85}, 1, 0},
    {0xB8, (uint8_t[]){0x21}, 1, 0},
    {0xB9, (uint8_t[]){0x10, 0x1F}, 2, 0},
    {0xBB, (uint8_t[]){0x03}, 1, 0},
    {0xBC, (uint8_t[]){0x00}, 1, 0},
    {0xC1, (uint8_t[]){0x78}, 1, 0},
    {0xC2, (uint8_t[]){0x78}, 1, 0},
    {0xD0, (uint8_t[]){0x88}, 1, 0},
    {0xE0, (uint8_t[]){0x00, 0x3A, 0x02}, 3, 0},
    {0xE1, (uint8_t[]){0x04, 0xA0, 0x00, 0xA0, 0x05, 0xA0, 0x00, 0xA0, 0x00, 0x40, 0x40}, 11, 0},
    {0xE2, (uint8_t[]){0x30, 0x00, 0x40, 0x40, 0x32, 0xA0, 0x00, 0xA0, 0x00, 0xA0, 0x00, 0xA0, 0x00}, 13, 0},
    {0xE3, (uint8_t[]){0x00, 0x00, 0x33, 0x33}, 4, 0},
    {0xE4, (uint8_t[]){0x44, 0x44}, 2, 0},
    {0xE5, (uint8_t[]){0x09, 0x2E, 0xA0, 0xA0, 0x0B, 0x30, 0xA0, 0xA0, 0x05, 0x2A, 0xA0, 0xA0, 0x07, 0x2C, 0xA0, 0xA0}, 16, 0},
    {0xE6, (uint8_t[]){0x00, 0x00, 0x33, 0x33}, 4, 0},
    {0xE7, (uint8_t[]){0x44, 0x44}, 2, 0},
    {0xE8, (uint8_t[]){0x08, 0x2D, 0xA0, 0xA0, 0x0A, 0x2F, 0xA0, 0xA0, 0x04, 0x29, 0xA0, 0xA0, 0x06, 0x2B, 0xA0, 0xA0}, 16, 0},
    {0xEB, (uint8_t[]){0x00, 0x00, 0x4E, 0x4E, 0x00, 0x00, 0x00}, 7, 0},
    {0xEC, (uint8_t[]){0x08, 0x01}, 2, 0},
    {0xED, (uint8_t[]){0xB0, 0x2B, 0x98, 0xA4, 0x56, 0x7F, 0xFF, 0xFF, 0xFF, 0xFF, 0xF7, 0x65, 0x4A, 0x89, 0xB2, 0x0B}, 16, 0},
    {0xEF, (uint8_t[]){0x08, 0x08, 0x08, 0x45, 0x3F, 0x54}, 6, 0},
    {0xFF, (uint8_t[]){0x77, 0x01, 0x00, 0x00, 0x00}, 5, 0},
    {0x11, (uint8_t[]){0x00}, 0, 120},
    {0x29, (uint8_t[]){0x00}, 0, 0},
};

void *board_i2c_bus(void)
{
    return s_bus;
}

static esp_err_t touch_init(void)
{
    i2c_master_bus_config_t bus = {
        .i2c_port = I2C_NUM_1,
        .sda_io_num = PIN_I2C_SDA,
        .scl_io_num = PIN_I2C_SCL,
        .clk_source = I2C_CLK_SRC_DEFAULT,
        .glitch_ignore_cnt = 7,
        .flags.enable_internal_pullup = true,
    };
    ESP_RETURN_ON_ERROR(i2c_new_master_bus(&bus, &s_bus), TAG, "i2c bus");
    // Touch reset and interrupt are not wired, so the GT911 keeps the address
    // it powered up with.
    uint8_t addr = GT911_ADDR;
    if (i2c_master_probe(s_bus, GT911_ADDR, 50) != ESP_OK) {
        if (i2c_master_probe(s_bus, GT911_ADDR_ALT, 50) != ESP_OK) {
            ESP_LOGE(TAG, "GT911 not found");
            return ESP_ERR_NOT_FOUND;
        }
        addr = GT911_ADDR_ALT;
    }
    ESP_LOGI(TAG, "GT911 at 0x%02x", addr);
    i2c_device_config_t dev = {
        .dev_addr_length = I2C_ADDR_BIT_LEN_7,
        .device_address = addr,
        .scl_speed_hz = 400000,
    };
    return i2c_master_bus_add_device(s_bus, &dev, &s_touch);
}

static esp_err_t panel_init(void)
{
    esp_ldo_channel_config_t ldo = {
        .chan_id = DSI_PHY_LDO_CHAN,
        .voltage_mv = DSI_PHY_LDO_MV,
    };
    ESP_RETURN_ON_ERROR(esp_ldo_acquire_channel(&ldo, &s_phy_ldo), TAG, "dsi phy ldo");

    esp_lcd_dsi_bus_config_t bus = {
        .bus_id = 0,
        .num_data_lanes = DSI_LANES,
        // 0 lets ESP-IDF pick the PHY clock for the chip revision
        // (PLL_F20M before v3, XTAL from v3 on).
        .phy_clk_src = 0,
        .lane_bit_rate_mbps = DSI_LANE_MBPS,
    };
    ESP_RETURN_ON_ERROR(esp_lcd_new_dsi_bus(&bus, &s_dsi), TAG, "dsi bus");

    esp_lcd_dbi_io_config_t dbi = {
        .virtual_channel = 0,
        .lcd_cmd_bits = 8,
        .lcd_param_bits = 8,
    };
    ESP_RETURN_ON_ERROR(esp_lcd_new_panel_io_dbi(s_dsi, &dbi, &s_io), TAG, "dbi io");

    esp_lcd_dpi_panel_config_t dpi = {
        .virtual_channel = 0,
        .dpi_clk_src = MIPI_DSI_DPI_CLK_SRC_DEFAULT,
        .dpi_clock_freq_mhz = DPI_CLOCK_MHZ,
        .pixel_format = LCD_COLOR_PIXEL_FORMAT_RGB565,
        .num_fbs = 1,
        .video_timing = {
            .h_size = BOARD_PANEL_W,
            .v_size = BOARD_PANEL_H,
            .hsync_back_porch = 42,
            .hsync_pulse_width = 12,
            .hsync_front_porch = 42,
            .vsync_back_porch = 2,
            .vsync_pulse_width = 8,
            .vsync_front_porch = 60,
        },
        // No DMA2D: draw_bitmap then copies synchronously, which is what
        // display.c's flush expects (it reuses its rotation buffer at once).
        .flags.use_dma2d = false,
    };
    st7701_vendor_config_t vendor = {
        .init_cmds = s_init_cmds,
        .init_cmds_size = sizeof(s_init_cmds) / sizeof(s_init_cmds[0]),
        .mipi_config = {
            .dsi_bus = s_dsi,
            .dpi_config = &dpi,
        },
        .flags.use_mipi_interface = 1,
    };
    esp_lcd_panel_dev_config_t dev = {
        .reset_gpio_num = PIN_LCD_RST,
        .rgb_ele_order = LCD_RGB_ELEMENT_ORDER_RGB,
        .bits_per_pixel = 16,
        .vendor_config = &vendor,
    };
    ESP_RETURN_ON_ERROR(esp_lcd_new_panel_st7701(s_io, &dev, &s_panel), TAG, "st7701");
    ESP_RETURN_ON_ERROR(esp_lcd_panel_reset(s_panel), TAG, "panel reset");
    ESP_RETURN_ON_ERROR(esp_lcd_panel_init(s_panel), TAG, "panel init");
    return ESP_OK;
}

static esp_err_t backlight_init(void)
{
    ledc_timer_config_t timer = {
        .speed_mode = LEDC_LOW_SPEED_MODE,
        .duty_resolution = LEDC_TIMER_10_BIT,
        .timer_num = BL_TIMER,
        .freq_hz = 5000,
        .clk_cfg = LEDC_AUTO_CLK,
    };
    ESP_RETURN_ON_ERROR(ledc_timer_config(&timer), TAG, "ledc timer");
    ledc_channel_config_t ch = {
        .gpio_num = PIN_BACKLIGHT,
        .speed_mode = LEDC_LOW_SPEED_MODE,
        .channel = BL_CHANNEL,
        .intr_type = LEDC_INTR_DISABLE,
        .timer_sel = BL_TIMER,
        .duty = 0,
        .hpoint = 0,
        // The backlight driver is active low.
        .flags.output_invert = 1,
    };
    return ledc_channel_config(&ch);
}

static void backlight_apply(void)
{
    uint32_t duty = s_on ? (uint32_t)BL_MAX * s_level / 100 : 0;
    if (ledc_set_duty(LEDC_LOW_SPEED_MODE, BL_CHANNEL, duty) != ESP_OK ||
        ledc_update_duty(LEDC_LOW_SPEED_MODE, BL_CHANNEL) != ESP_OK) {
        ESP_LOGW(TAG, "backlight write failed");
    }
}

esp_err_t board_init(void)
{
    ESP_RETURN_ON_ERROR(backlight_init(), TAG, "backlight");
    ESP_RETURN_ON_ERROR(panel_init(), TAG, "panel");
    // A missing touch controller should not stop the dashboard from showing.
    if (touch_init() != ESP_OK) ESP_LOGW(TAG, "touch unavailable");
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
    s_on = on;
    backlight_apply();
}

void board_backlight_set(uint8_t pct)
{
    if (pct < 1) pct = 1;
    if (pct > 100) pct = 100;
    if (pct == s_level) return;
    s_level = pct;
    if (s_on) backlight_apply();
}

bool board_has_dimming(void)
{
    return true;
}

static esp_err_t gt911_read(uint16_t reg, uint8_t *buf, size_t len)
{
    uint8_t r[2] = {reg >> 8, reg & 0xff};
    return i2c_master_transmit_receive(s_touch, r, 2, buf, len, 50);
}

bool board_touch_read(uint16_t *x, uint16_t *y)
{
    *x = s_tx;
    *y = s_ty;
    if (!s_touch) return false;
    uint8_t status;
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
                if (s_tx >= BOARD_PANEL_W) s_tx = BOARD_PANEL_W - 1;
                if (s_ty >= BOARD_PANEL_H) s_ty = BOARD_PANEL_H - 1;
            }
        }
        uint8_t clear[3] = {GT911_STATUS >> 8, GT911_STATUS & 0xff, 0};
        i2c_master_transmit(s_touch, clear, sizeof(clear), 50);
    }
    *x = s_tx;
    *y = s_ty;
    return s_touch_down;
}

bool board_snapshot(uint16_t *dst)
{
    void *fb = NULL;
    if (esp_lcd_dpi_panel_get_frame_buffer(s_panel, 1, &fb) != ESP_OK || !fb) return false;
    // The frame buffer is the portrait panel; turn it back into the 800x480 UI.
    const uint16_t *src = fb;
    for (int ly = 0; ly < BOARD_HEIGHT; ly++) {
        for (int lx = 0; lx < BOARD_WIDTH; lx++) {
            uint16_t nx, ny;
            board_logical_to_panel(lx, ly, &nx, &ny);
            dst[ly * BOARD_WIDTH + lx] = src[ny * BOARD_PANEL_W + nx];
        }
    }
    return true;
}
