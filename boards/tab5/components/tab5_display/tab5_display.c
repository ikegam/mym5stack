/*
 * M5Stack Tab5 MIPI-DSI display bring-up.
 *
 * Trimmed from Espressif's m5stack_tab5 BSP (Apache-2.0,
 * https://github.com/espressif/esp-bsp/tree/master/bsp/m5stack_tab5):
 * same power/IO-expander sequence, board-variant probe, DSI/DPI timings and
 * panel init tables, without LVGL. Rust draws straight into the DPI frame
 * buffer and calls tab5_display_flush().
 */
#include "tab5_display.h"

#include "driver/gpio.h"
#include "driver/i2c_master.h"
#include "driver/ledc.h"
#include "esp_check.h"
#include "esp_io_expander_pi4ioe5v6408.h"
#include "esp_lcd_ili9881c.h"
#include "esp_lcd_mipi_dsi.h"
#include "esp_lcd_panel_io.h"
#include "esp_lcd_panel_ops.h"
#include "esp_lcd_st7123.h"
#include "esp_lcd_touch_gt911.h"
#include "esp_lcd_touch_st7123.h"
#include "esp_ldo_regulator.h"
#include "esp_log.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"

#include "priv/init_ili9881c.h"
#include "priv/init_st7121.h"
#include "priv/init_st7123.h"

static const char *TAG = "tab5_display";

#define I2C_PORT        1
#define I2C_SDA         31
#define I2C_SCL         32
#define BACKLIGHT_GPIO  22
#define BACKLIGHT_CH    LEDC_CHANNEL_0
#define EXP_LCD_EN      IO_EXPANDER_PIN_NUM_4
#define EXP_TOUCH_EN    IO_EXPANDER_PIN_NUM_5
#define DSI_PHY_LDO     3
#define DSI_PHY_LDO_MV  2500
#define DSI_LANES       2
#define DSI_MBPS        1000
#define DSI_MBPS_ST7121 965
/* As in the BSP. 40 MHz (to leave the frame buffer's PSRAM bandwidth to the
 * LLM) left the ST7121 panel dark. */
#define DPI_MHZ_ST712X  70

static i2c_master_bus_handle_t i2c_bus;
static esp_lcd_panel_handle_t panel;
static tab5_lcd_t variant;
static esp_lcd_touch_handle_t touch;
static void *frame_buffer;

esp_err_t tab5_i2c_bus(void **bus)
{
    if (!i2c_bus) {
        const i2c_master_bus_config_t cfg = {
            .i2c_port = I2C_PORT,
            .sda_io_num = I2C_SDA,
            .scl_io_num = I2C_SCL,
            .clk_source = I2C_CLK_SRC_DEFAULT,
            .flags.enable_internal_pullup = true,
        };
        ESP_RETURN_ON_ERROR(i2c_new_master_bus(&cfg, &i2c_bus), TAG, "i2c bus");
    }
    *bus = i2c_bus;
    return ESP_OK;
}

static esp_err_t backlight_init(void)
{
    const ledc_timer_config_t timer = {
        .speed_mode = LEDC_LOW_SPEED_MODE,
        .duty_resolution = LEDC_TIMER_10_BIT,
        .timer_num = LEDC_TIMER_0,
        .freq_hz = 5000,
        .clk_cfg = LEDC_AUTO_CLK,
    };
    const ledc_channel_config_t ch = {
        .gpio_num = BACKLIGHT_GPIO,
        .speed_mode = LEDC_LOW_SPEED_MODE,
        .channel = BACKLIGHT_CH,
        .intr_type = LEDC_INTR_DISABLE,
        .timer_sel = LEDC_TIMER_0,
        .duty = 0,
        .hpoint = 0,
    };
    ESP_RETURN_ON_ERROR(ledc_timer_config(&timer), TAG, "ledc timer");
    return ledc_channel_config(&ch);
}

esp_err_t tab5_display_brightness(int percent)
{
    percent = percent < 0 ? 0 : percent > 100 ? 100 : percent;
    ESP_RETURN_ON_ERROR(ledc_set_duty(LEDC_LOW_SPEED_MODE, BACKLIGHT_CH, 1023 * percent / 100), TAG, "duty");
    return ledc_update_duty(LEDC_LOW_SPEED_MODE, BACKLIGHT_CH);
}

/* Which LCD/touch pair this unit has, told apart by the touch controller. */
static tab5_lcd_t probe_variant(esp_io_expander_handle_t exp)
{
    esp_io_expander_set_dir(exp, EXP_TOUCH_EN, IO_EXPANDER_OUTPUT);
    esp_io_expander_set_level(exp, EXP_TOUCH_EN, 1);
    esp_io_expander_set_output_mode(exp, EXP_TOUCH_EN, IO_EXPANDER_OUTPUT_MODE_PUSH_PULL);
    vTaskDelay(pdMS_TO_TICKS(500));

    if (i2c_master_probe(i2c_bus, ESP_LCD_TOUCH_IO_I2C_ST7123_ADDRESS, 100) == ESP_OK) {
        esp_lcd_panel_io_handle_t io = NULL;
        esp_lcd_panel_io_i2c_config_t cfg = ESP_LCD_TOUCH_IO_I2C_ST7123_CONFIG();
        uint8_t fw = 0;
        if (esp_lcd_new_panel_io_i2c(i2c_bus, &cfg, &io) != ESP_OK) {
            return TAB5_LCD_UNKNOWN;
        }
        esp_err_t err = esp_lcd_panel_io_rx_param(io, 0x0000, &fw, 1);
        esp_lcd_panel_io_del(io);
        if (err != ESP_OK) {
            return TAB5_LCD_UNKNOWN;
        }
        ESP_LOGI(TAG, "ST712x touch firmware %u", fw);
        return fw == 1 ? TAB5_LCD_ST7121 : fw == 3 ? TAB5_LCD_ST7123 : TAB5_LCD_UNKNOWN;
    }
    if (i2c_master_probe(i2c_bus, ESP_LCD_TOUCH_IO_I2C_GT911_ADDRESS_BACKUP, 100) == ESP_OK) {
        return TAB5_LCD_ILI9881C;
    }
    return TAB5_LCD_UNKNOWN;
}

esp_err_t tab5_display_init(void **fb, tab5_lcd_t *variant_out)
{
    void *bus;
    ESP_RETURN_ON_ERROR(tab5_i2c_bus(&bus), TAG, "i2c");

    esp_io_expander_handle_t exp = NULL;
    ESP_RETURN_ON_ERROR(esp_io_expander_new_i2c_pi4ioe5v6408(i2c_bus, ESP_IO_EXPANDER_I2C_PI4IOE5V6408_ADDRESS_LOW,
                        &exp), TAG, "io expander");
    /* LCD_EN is released (pulled up, open drain) rather than driven high. */
    esp_io_expander_set_pullupdown(exp, EXP_LCD_EN, IO_EXPANDER_PULL_UP);
    esp_io_expander_set_dir(exp, EXP_LCD_EN, IO_EXPANDER_INPUT);
    esp_io_expander_set_output_mode(exp, EXP_LCD_EN, IO_EXPANDER_OUTPUT_MODE_OPEN_DRAIN);

    ESP_RETURN_ON_ERROR(backlight_init(), TAG, "backlight");

    esp_ldo_channel_handle_t ldo = NULL;
    const esp_ldo_channel_config_t ldo_cfg = { .chan_id = DSI_PHY_LDO, .voltage_mv = DSI_PHY_LDO_MV };
    ESP_RETURN_ON_ERROR(esp_ldo_acquire_channel(&ldo_cfg, &ldo), TAG, "dsi phy ldo");

    tab5_lcd_t v = probe_variant(exp);
    *variant_out = v;
    variant = v;
    ESP_RETURN_ON_FALSE(v != TAB5_LCD_UNKNOWN, ESP_ERR_NOT_SUPPORTED, TAG, "unknown Tab5 LCD variant");

    esp_lcd_dsi_bus_handle_t dsi = NULL;
    const esp_lcd_dsi_bus_config_t bus_cfg = {
        .bus_id = 0,
        .num_data_lanes = DSI_LANES,
        .phy_clk_src = 0,
        .lane_bit_rate_mbps = v == TAB5_LCD_ST7121 ? DSI_MBPS_ST7121 : DSI_MBPS,
    };
    ESP_RETURN_ON_ERROR(esp_lcd_new_dsi_bus(&bus_cfg, &dsi), TAG, "dsi bus");

    esp_lcd_panel_io_handle_t io = NULL;
    const esp_lcd_dbi_io_config_t dbi_cfg = { .virtual_channel = 0, .lcd_cmd_bits = 8, .lcd_param_bits = 8 };
    ESP_RETURN_ON_ERROR(esp_lcd_new_panel_io_dbi(dsi, &dbi_cfg, &io), TAG, "dbi io");

    esp_lcd_dpi_panel_config_t dpi = {
        .virtual_channel = 0,
        .dpi_clk_src = MIPI_DSI_DPI_CLK_SRC_DEFAULT,
        .in_color_format = LCD_COLOR_FMT_RGB565,
        .num_fbs = 1,
        .video_timing = { .h_size = TAB5_LCD_W, .v_size = TAB5_LCD_H },
    };
    if (v == TAB5_LCD_ILI9881C) {
        dpi.dpi_clock_freq_mhz = 60;
        dpi.video_timing.hsync_back_porch = 140;
        dpi.video_timing.hsync_pulse_width = 40;
        dpi.video_timing.hsync_front_porch = 40;
        dpi.video_timing.vsync_back_porch = 20;
        dpi.video_timing.vsync_pulse_width = 4;
        dpi.video_timing.vsync_front_porch = 20;
    } else if (v == TAB5_LCD_ST7123) {
        dpi.dpi_clock_freq_mhz = DPI_MHZ_ST712X;
        dpi.video_timing.hsync_back_porch = 40;
        dpi.video_timing.hsync_pulse_width = 2;
        dpi.video_timing.hsync_front_porch = 40;
        dpi.video_timing.vsync_back_porch = 8;
        dpi.video_timing.vsync_pulse_width = 2;
        dpi.video_timing.vsync_front_porch = 220;
    } else {
        dpi.dpi_clock_freq_mhz = DPI_MHZ_ST712X;
        dpi.video_timing.hsync_back_porch = 40;
        dpi.video_timing.hsync_pulse_width = 2;
        dpi.video_timing.hsync_front_porch = 40;
        dpi.video_timing.vsync_back_porch = 24;
        dpi.video_timing.vsync_pulse_width = 20;
        dpi.video_timing.vsync_front_porch = 200;
    }

    const ili9881c_vendor_config_t ili_cfg = {
        .init_cmds = disp_init_data_ili9881c,
        .init_cmds_size = sizeof(disp_init_data_ili9881c) / sizeof(disp_init_data_ili9881c[0]),
        .mipi_config = { .dsi_bus = dsi, .dpi_config = &dpi, .lane_num = DSI_LANES },
    };
    const st7123_vendor_config_t st7123_cfg = {
        .init_cmds = disp_init_data_st7123,
        .init_cmds_size = sizeof(disp_init_data_st7123) / sizeof(disp_init_data_st7123[0]),
        .mipi_config = { .dsi_bus = dsi, .dpi_config = &dpi },
    };
    const st7123_vendor_config_t st7121_cfg = {
        .init_cmds = disp_init_data_st7121,
        .init_cmds_size = sizeof(disp_init_data_st7121) / sizeof(disp_init_data_st7121[0]),
        .mipi_config = { .dsi_bus = dsi, .dpi_config = &dpi },
    };
    esp_lcd_panel_dev_config_t panel_cfg = {
        .reset_gpio_num = -1,
        .rgb_ele_order = LCD_RGB_ELEMENT_ORDER_RGB,
        .bits_per_pixel = 16,
    };
    if (v == TAB5_LCD_ILI9881C) {
        panel_cfg.vendor_config = (void *)&ili_cfg;
        ESP_RETURN_ON_ERROR(esp_lcd_new_panel_ili9881c(io, &panel_cfg, &panel), TAG, "ili9881c");
    } else {
        panel_cfg.vendor_config = (void *)(v == TAB5_LCD_ST7123 ? &st7123_cfg : &st7121_cfg);
        ESP_RETURN_ON_ERROR(esp_lcd_new_panel_st7123(io, &panel_cfg, &panel), TAG, "st7123");
    }
    ESP_RETURN_ON_ERROR(esp_lcd_panel_reset(panel), TAG, "reset");
    ESP_RETURN_ON_ERROR(esp_lcd_panel_init(panel), TAG, "init");
    esp_lcd_panel_disp_on_off(panel, true);
    ESP_RETURN_ON_ERROR(esp_lcd_dpi_panel_get_frame_buffer(panel, 1, &frame_buffer), TAG, "frame buffer");
    *fb = frame_buffer;
    ESP_LOGI(TAG, "display up: variant %d, %dx%d", v, TAB5_LCD_W, TAB5_LCD_H);
    return ESP_OK;
}

/* Touch was already powered by probe_variant(). */
esp_err_t tab5_touch_init(void)
{
    ESP_RETURN_ON_FALSE(variant != TAB5_LCD_UNKNOWN, ESP_ERR_INVALID_STATE, TAG, "display not initialized");
    const esp_lcd_touch_config_t cfg = {
        .x_max = TAB5_LCD_W,
        .y_max = TAB5_LCD_H,
        .rst_gpio_num = -1,
        .int_gpio_num = 23,
    };
    esp_lcd_panel_io_handle_t io = NULL;
    if (variant == TAB5_LCD_ILI9881C) {
        /* GT911 needs its INT line held low (BSP workaround for board v1). */
        const gpio_config_t int_cfg = {
            .mode = GPIO_MODE_OUTPUT,
            .pull_up_en = 1,
            .pin_bit_mask = BIT64(23),
        };
        gpio_config(&int_cfg);
        gpio_set_level(23, 0);
        esp_lcd_touch_config_t gt = cfg;
        gt.int_gpio_num = -1;
        esp_lcd_panel_io_i2c_config_t io_cfg = ESP_LCD_TOUCH_IO_I2C_GT911_CONFIG();
        io_cfg.dev_addr = ESP_LCD_TOUCH_IO_I2C_GT911_ADDRESS_BACKUP;
        ESP_RETURN_ON_ERROR(esp_lcd_new_panel_io_i2c(i2c_bus, &io_cfg, &io), TAG, "touch io");
        return esp_lcd_touch_new_i2c_gt911(io, &gt, &touch);
    }
    const esp_lcd_panel_io_i2c_config_t io_cfg = ESP_LCD_TOUCH_IO_I2C_ST7123_CONFIG();
    ESP_RETURN_ON_ERROR(esp_lcd_new_panel_io_i2c(i2c_bus, &io_cfg, &io), TAG, "touch io");
    return esp_lcd_touch_new_i2c_st7123(io, &cfg, &touch);
}

bool tab5_touch_read(uint16_t *x, uint16_t *y)
{
    if (!touch || esp_lcd_touch_read_data(touch) != ESP_OK) {
        return false;
    }
    esp_lcd_touch_point_data_t p[1];
    uint8_t n = 0;
    if (esp_lcd_touch_get_data(touch, p, &n, 1) != ESP_OK || n == 0) {
        return false;
    }
    *x = p[0].x;
    *y = p[0].y;
    return true;
}

/* IO expander 2 (0x44) as M5Stack's Tab5 firmware sets it up: after reset
 * every output is high-impedance, so the generic driver's level writes
 * never reach WLAN_PWR_EN. Outputs: P0 WLAN_PWR_EN, P3 USB5V_EN. */
esp_err_t tab5_wifi_power(bool on)
{
    void *bus;
    ESP_RETURN_ON_ERROR(tab5_i2c_bus(&bus), TAG, "i2c");
    static i2c_master_dev_handle_t exp2;
    if (!exp2) {
        const i2c_device_config_t cfg = {
            .dev_addr_length = I2C_ADDR_BIT_LEN_7,
            .device_address = 0x44,
            .scl_speed_hz = 400000,
        };
        ESP_RETURN_ON_ERROR(i2c_master_bus_add_device(i2c_bus, &cfg, &exp2), TAG, "io expander 2");
        const uint8_t init[][2] = {
            {0x01, 0xFF},       /* chip reset */
            {0x03, 0b10111001}, /* direction, 1 = output */
            {0x07, 0b00000110}, /* high-Z only on unused outputs */
            {0x0D, 0b10111001}, /* pull select, 1 = up */
            {0x0B, 0b11111001}, /* pull enable */
        };
        for (size_t i = 0; i < sizeof(init) / sizeof(init[0]); i++) {
            ESP_RETURN_ON_ERROR(i2c_master_transmit(exp2, init[i], 2, 50), TAG, "io expander 2 init");
        }
    }
    const uint8_t out[2] = {0x05, on ? 0b00001001 : 0b00001000};
    ESP_RETURN_ON_ERROR(i2c_master_transmit(exp2, out, 2, 50), TAG, "io expander 2 out");
    uint8_t regs[] = {0x03, 0x05, 0x07, 0x0F}, val[4] = {0};
    for (int i = 0; i < 4; i++) {
        i2c_master_transmit_receive(exp2, &regs[i], 1, &val[i], 1, 50);
    }
    ESP_LOGI(TAG, "expander2: dir=%02x out=%02x hiz=%02x in=%02x", val[0], val[1], val[2], val[3]);
    return ESP_OK;
}

esp_err_t tab5_display_flush(int y0, int y1)
{
    /* Drawing from the frame buffer itself only writes back the CPU cache. */
    uint8_t *fb = frame_buffer;
    return esp_lcd_panel_draw_bitmap(panel, 0, y0, TAB5_LCD_W, y1, fb + (size_t)y0 * TAB5_LCD_W * 2);
}
