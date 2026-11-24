# One board file per target: board.c for the ESP32-S3 4.3B, board_p4.c for the
# ESP32-P4 4.3 (its Wi-Fi comes from esp_wifi_remote, which provides esp_wifi).
if(IDF_TARGET STREQUAL "esp32p4")
    set(board_src "board_p4.c")
    set(board_requires esp_driver_ledc esp_hw_support)
else()
    set(board_src "board.c")
    set(board_requires "")
endif()

idf_component_register(
    SRCS "main.c" "${board_src}" "hubcfg.c" "display.c" "net.c" "hub_server.c" "weather.c"
         "ui.c" "ui_home.c" "ui_stats.c" "ui_media.c" "ui_calendar.c" "ui_weather.c" "ui_shortcuts.c" "ui_claude.c" "ui_printer.c" "ui_alert.c" "ui_timer.c" "ui_games.c" "ui_net.c" "ui_github.c" "ui_saver.c" "ui_update.c" "ota.c" "logos.c" "lv_mem_psram.c"
    INCLUDE_DIRS "."
    EMBED_FILES "fonts/MaterialIcons-Regular.ttf" "fonts/Inter-Medium.ttf" "fonts/Inter-SemiBold.ttf"
    REQUIRES esp_lcd esp_driver_i2c esp_driver_gpio esp_wifi esp_netif esp_event nvs_flash
             esp_http_server esp_http_client esp-tls json esp_timer app_update esp_app_format
             ${board_requires}
)

# Board variant for the release image picker (build.sh: s3, p4rev1, p4rev3).
if(DEFINED HUB_VARIANT)
    target_compile_definitions(${COMPONENT_LIB} PRIVATE HUB_VARIANT="${HUB_VARIANT}")
endif()

# LVGL calls lv_malloc_core() & co. from lv_mem_psram.c; nothing in main refers
# to them, so make the linker keep that object.
target_link_libraries(${COMPONENT_LIB} INTERFACE "-u lv_malloc_core")
