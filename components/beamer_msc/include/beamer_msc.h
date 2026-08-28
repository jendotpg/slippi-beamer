/*
 * USB Mass Storage for the Beamer: descriptors, the tud_msc_* callbacks, and
 * the timing ring they feed.
 *
 * This is the only raw FFI in the project. We use it instead of esp_tinyusb
 * so that we can implement our own logging for debug purposes. It's lowkey
 * vibe-coded - if something seems really stupid it may well be (I'm not much
 * of a C dev and certainly not on embedded)!
 */
#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "esp_err.h"
#include "freertos/FreeRTOS.h"
#include "freertos/semphr.h"
#include "sdmmc_cmd.h"

#include "diskio_impl.h"
#include "diskio_sdmmc.h"
#include "driver/sdmmc_host.h"
#include "esp_vfs_fat.h"
#include "ff.h"

#ifdef __cplusplus
extern "C"
{
#endif
    typedef enum
    {
        BEAMER_MSC_OP_READ = 0,
        BEAMER_MSC_OP_WRITE = 1,
        BEAMER_MSC_OP_FLUSH = 2,
    } beamer_msc_op_t;

#define BEAMER_SCSI_CMD_SYNCHRONIZE_CACHE_10 0x35

    typedef struct
    {
        uint32_t count;
        uint8_t op;
    } beamer_msc_unsup_t;

    typedef struct
    {
        uint32_t start_us; // wraps
        uint32_t dur_us;
        uint32_t lba;
        uint16_t blocks;
        uint8_t op;
        uint8_t err; // 0 on success
    } beamer_msc_sample_t;

    esp_err_t beamer_msc_install(sdmmc_card_t *card, SemaphoreHandle_t lock, const char *serial);

    bool beamer_msc_mounted(void);
    bool beamer_msc_suspended(void);

    uint32_t beamer_msc_reads_ok(void);
    uint32_t beamer_msc_mounts(void);
    uint32_t beamer_msc_umounts(void);
    uint32_t beamer_msc_writes_ok(void);

    int beamer_msc_first_err(void);

    bool beamer_msc_eject_seen(void);

    uint32_t beamer_msc_maxlun_asks(void);

    int64_t beamer_msc_last_cbw_us(void);

    size_t beamer_msc_census(beamer_msc_unsup_t *out, size_t max);

    size_t beamer_msc_unsupported(beamer_msc_unsup_t *out, size_t max);

    bool beamer_msc_host_owns(void);

    bool beamer_msc_take_dirty(void);
    bool beamer_msc_take_eject(void);
    bool beamer_msc_take_load(void);

    void beamer_msc_ring_push(uint8_t op, uint32_t lba, uint16_t blocks, int64_t t0, int64_t t1,
                              esp_err_t err);

    int64_t beamer_msc_bind_time_us(void);
    size_t beamer_msc_drain(beamer_msc_sample_t *out, size_t max);
    uint32_t beamer_msc_dropped(void);

    void beamer_msc_set_media(bool present);
    bool beamer_msc_media_present(void);

    void beamer_log_install(void);
    void beamer_log_push(const char *s, size_t n);

    size_t beamer_log_drain(char *out, size_t max);
    uint32_t beamer_log_dropped(void);

    typedef enum
    {
        BEAMER_WBC_WRITEBACK = 0,
        BEAMER_WBC_WRITETHROUGH = 1,
        BEAMER_WBC_REFUSE = 2,
    } beamer_wbc_policy_t;

    esp_err_t beamer_wbc_start(sdmmc_card_t *card, SemaphoreHandle_t lock);
    esp_err_t beamer_wbc_write(uint32_t lba, const void *buf, size_t count);
    esp_err_t beamer_wbc_read(uint32_t lba, void *buf, size_t count);
    esp_err_t beamer_wbc_flush_all(void);

    void beamer_wbc_set_policy(beamer_wbc_policy_t policy);
    beamer_wbc_policy_t beamer_wbc_policy(void);

    uint32_t beamer_wbc_dirty(void);
    uint32_t beamer_wbc_high_water(void);
    uint32_t beamer_wbc_capacity(void);
    uint32_t beamer_wbc_stalls(void);

    esp_err_t beamer_sd_init(sdmmc_card_t **out_card, SemaphoreHandle_t *out_lock);
    uint64_t beamer_sd_bytes(const sdmmc_card_t *card);

    typedef struct
    {
        uint32_t start;
        uint32_t sectors;
        uint8_t type;
    } beamer_part_t;

#define BEAMER_MAX_VOLUME_SECTORS (16u * 1024u * 1024u * 1024u / 512u)

    void beamer_msc_set_visible(uint32_t sectors);

    esp_err_t beamer_sd_probe_partition(sdmmc_card_t *card, SemaphoreHandle_t lock,
                                        uint32_t max_sectors, beamer_part_t *out);
    esp_err_t beamer_fat_ro_register(uint8_t pdrv, sdmmc_card_t *card);

    bool beamer_panic_take(char *out, size_t len);

    uint32_t beamer_boot_count(void);
    size_t beamer_backtrace(char *out, size_t len);

#ifdef __cplusplus
}
#endif
