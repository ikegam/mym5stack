#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "esp_err.h"

#ifdef __cplusplus
extern "C" {
#endif

#define TAB5_LCD_W 720
#define TAB5_LCD_H 1280

typedef enum {
    TAB5_LCD_UNKNOWN = 0,
    TAB5_LCD_ILI9881C = 1,
    TAB5_LCD_ST7123 = 2,
    TAB5_LCD_ST7121 = 3,
} tab5_lcd_t;

/* Shared I2C master bus (port 1, SDA 31 / SCL 32). */
esp_err_t tab5_i2c_bus(void **bus);

/* Powers up and initializes the panel; returns its RGB565 frame buffer
 * (TAB5_LCD_W x TAB5_LCD_H, in PSRAM). The backlight starts off. */
esp_err_t tab5_display_init(void **fb, tab5_lcd_t *variant);

/* Makes rows y0..y1 of the frame buffer visible (cache write-back). */
esp_err_t tab5_display_flush(int y0, int y1);

esp_err_t tab5_display_brightness(int percent);

/* Touch panel matching the probed LCD variant; call after tab5_display_init(). */
esp_err_t tab5_touch_init(void);

/* Current touch point, if the screen is being touched. */
bool tab5_touch_read(uint16_t *x, uint16_t *y);

/* Powers the ESP32-C6 WiFi co-processor (IO expander 2, pin 0). */
esp_err_t tab5_wifi_power(bool on);

#ifdef __cplusplus
}
#endif
