# Emscripten build of the board UI. Run through build.sh (pinned emsdk image).
cmake_minimum_required(VERSION 3.16)
project(deskhub_sim C)

set(MAIN ${CMAKE_CURRENT_SOURCE_DIR}/../main)
set(DEPS ${CMAKE_CURRENT_SOURCE_DIR}/.deps)
set(LVGL ${DEPS}/lvgl)

file(GLOB_RECURSE LVGL_SRC ${LVGL}/src/*.c)
add_library(lvgl STATIC ${LVGL_SRC})
target_include_directories(lvgl PUBLIC ${LVGL} ${CMAKE_CURRENT_SOURCE_DIR})
target_compile_definitions(lvgl PUBLIC LV_CONF_INCLUDE_SIMPLE)
target_compile_options(lvgl PRIVATE -O2 -w)

set(UI_SRC
    ui.c ui_home.c ui_stats.c ui_media.c ui_calendar.c ui_weather.c ui_shortcuts.c ui_claude.c
    ui_printer.c ui_alert.c ui_timer.c ui_games.c ui_net.c ui_github.c ui_saver.c ui_update.c
    logos.c hub_dispatch.c weather_data.c)
list(TRANSFORM UI_SRC PREPEND ${MAIN}/)

add_executable(deskhub_sim
    ${UI_SRC} sim_main.c sim_stubs.c ${CMAKE_CURRENT_BINARY_DIR}/sim_fonts.c ${DEPS}/cJSON/cJSON.c)
target_include_directories(deskhub_sim PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/shim ${MAIN} ${DEPS}/cJSON)
target_compile_definitions(deskhub_sim PRIVATE HUB_SIM)
target_compile_options(deskhub_sim PRIVATE -O2)
target_link_libraries(deskhub_sim lvgl)
set_target_properties(deskhub_sim PROPERTIES SUFFIX ".js")
target_link_options(deskhub_sim PRIVATE
    -O2
    -sMODULARIZE=1 -sEXPORT_NAME=createDeskhubSim -sENVIRONMENT=web
    -sALLOW_MEMORY_GROWTH=1 -sINITIAL_MEMORY=64MB -sSTACK_SIZE=1MB
    -sEXPORTED_FUNCTIONS=_malloc,_free,_sim_init,_sim_step,_sim_pixels,_sim_take_dirty,_sim_pointer,_sim_show_home,_sim_json,_sim_binary,_sim_weather_url,_sim_weather
    -sEXPORTED_RUNTIME_METHODS=HEAPU8,UTF8ToString,stringToNewUTF8)

# The board's fonts (EMBED_FILES there) as C arrays.
add_custom_command(
    OUTPUT ${CMAKE_CURRENT_BINARY_DIR}/sim_fonts.c
    COMMAND python3 ${CMAKE_CURRENT_SOURCE_DIR}/embed_fonts.py ${MAIN}/fonts ${CMAKE_CURRENT_BINARY_DIR}/sim_fonts.c
    DEPENDS ${CMAKE_CURRENT_SOURCE_DIR}/embed_fonts.py
            ${MAIN}/fonts/MaterialIcons-Regular.ttf ${MAIN}/fonts/Inter-Medium.ttf ${MAIN}/fonts/Inter-SemiBold.ttf)
