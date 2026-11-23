cmake_minimum_required(VERSION 3.16)
# Two boards: esp32s3 (Waveshare 4.3B, the default) and esp32p4 (Waveshare
# ESP32-P4-WIFI6-Touch-LCD-4.3). Without IDF_TARGET set, build for the 4.3B as before.
if(NOT DEFINED ENV{IDF_TARGET} AND NOT DEFINED IDF_TARGET AND NOT EXISTS "${CMAKE_CURRENT_LIST_DIR}/sdkconfig")
    set(ENV{IDF_TARGET} esp32s3)
endif()
include($ENV{IDF_PATH}/tools/cmake/project.cmake)
project(deskhub)
