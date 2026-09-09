idf_component_register(
    SRCS "beamer_msc.c" "beamer_panic.c" "beamer_tombstone.c" "beamer_wbc.c" "beamer_log.c" "beamer_fat.c" "beamer_oom.c"
    INCLUDE_DIRS "include"
    REQUIRES sdmmc fatfs vfs
    PRIV_REQUIRES usb esp_timer driver espcoredump esp_system
)

idf_build_get_property(build_components BUILD_COMPONENTS)
if(tinyusb IN_LIST build_components)
    set(tinyusb_name tinyusb)
else()
    set(tinyusb_name espressif__tinyusb)
endif()

idf_component_get_property(tusb_lib ${tinyusb_name} COMPONENT_LIB)
target_include_directories(${tusb_lib} PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include")

target_link_libraries(${COMPONENT_LIB} INTERFACE "-Wl,--wrap=esp_panic_handler")
idf_component_get_property(lwip_lib lwip COMPONENT_LIB)
target_compile_definitions(${lwip_lib} PRIVATE
    "TCP_SND_QUEUELEN=((2 * (TCP_SND_BUF) + (TCP_MSS - 1))/(TCP_MSS))") # set TCP_SND_QUEUELEN against TCP_SND_BUF
