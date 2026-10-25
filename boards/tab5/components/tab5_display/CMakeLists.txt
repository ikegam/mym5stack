idf_component_register(
    SRCS "tab5_display.c"
    INCLUDE_DIRS "include"
    PRIV_INCLUDE_DIRS "."
    PRIV_REQUIRES esp_lcd esp_driver_gpio esp_driver_i2c esp_driver_ledc esp_hw_support
)
