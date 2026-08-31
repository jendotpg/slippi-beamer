/*
 * See include/beamer_msc.h for documentation
 */

#include "beamer_msc.h"

#include <stdatomic.h>
#include <string.h>

#include "esp_check.h"
#include "esp_log.h"
#include "esp_private/usb_phy.h"
#include "esp_timer.h"
#include "freertos/task.h"
#include "tusb.h"

static const char *TAG = "beamer_msc";

// we're a SanDisk Cruzer Blade :D yay :D
#define BEAMER_VID 0x0781
#define BEAMER_PID 0x5567

enum
{
    STR_LANGID = 0,
    STR_MANUFACTURER,
    STR_PRODUCT,
    STR_SERIAL,
    STR_CONFIG,
    STR_COUNT,
};

#define EPNUM_MSC_OUT 0x01
#define EPNUM_MSC_IN 0x81
#define BEAMER_CONFIG_TOTAL_LEN (TUD_CONFIG_DESC_LEN + TUD_MSC_DESC_LEN)

static const tusb_desc_device_t s_desc_device = {
    .bLength = sizeof(tusb_desc_device_t),
    .bDescriptorType = TUSB_DESC_DEVICE,
    .bcdUSB = 0x0200,
    .bDeviceClass = 0x00,
    .bDeviceSubClass = 0x00,
    .bDeviceProtocol = 0x00,
    .bMaxPacketSize0 = CFG_TUD_ENDPOINT0_SIZE,
    .idVendor = BEAMER_VID,
    .idProduct = BEAMER_PID,
    .bcdDevice = 0x0100,
    .iManufacturer = STR_MANUFACTURER,
    .iProduct = STR_PRODUCT,
    .iSerialNumber = STR_SERIAL,
    .bNumConfigurations = 0x01,
};

static const uint8_t s_desc_config[] = {
    TUD_CONFIG_DESCRIPTOR(1, 1, STR_CONFIG, BEAMER_CONFIG_TOTAL_LEN, 0x00, 500),
    TUD_MSC_DESCRIPTOR(0, 0, EPNUM_MSC_OUT, EPNUM_MSC_IN, 64),
};

static char s_serial[48] = "BEAMER-UNSET";

static const char *const s_desc_strings[STR_COUNT] = {
    [STR_LANGID] = (const char[]){0x09, 0x04},
    [STR_MANUFACTURER] = "SanDisk",
    [STR_PRODUCT] = "Cruzer Blade",
    [STR_SERIAL] = s_serial,
    [STR_CONFIG] = "Config 1",
};

const uint8_t *tud_descriptor_device_cb(void)
{
    return (const uint8_t *)&s_desc_device;
}

const uint8_t *tud_descriptor_configuration_cb(uint8_t index)
{
    (void)index;
    return s_desc_config;
}

const uint16_t *tud_descriptor_string_cb(uint8_t index, uint16_t langid)
{
    (void)langid;
    static uint16_t buf[64];
    size_t chars;

    if (index == STR_LANGID)
    {
        memcpy(&buf[1], s_desc_strings[STR_LANGID], 2);
        chars = 1;
    }
    else
    {
        if (index >= STR_COUNT)
        {
            return NULL;
        }
        const char *str = s_desc_strings[index];
        chars = strlen(str);
        if (chars > (sizeof(buf) / sizeof(buf[0])) - 1)
        {
            chars = (sizeof(buf) / sizeof(buf[0])) - 1;
        }
        for (size_t i = 0; i < chars; i++)
        {
            buf[1 + i] = (uint16_t)str[i];
        }
    }

    buf[0] = (uint16_t)((TUSB_DESC_STRING << 8) | (2 * chars + 2));
    return buf;
}

static sdmmc_card_t *s_card;
static SemaphoreHandle_t s_lock;
static atomic_bool s_media_present;
static atomic_bool s_load;
static atomic_bool s_dirty;
static atomic_bool s_eject;
static int64_t s_bind_us;

static atomic_uint s_reads_ok;
static atomic_int s_first_err;
static atomic_bool s_locked; // locked means "received PREVENT ALLOW MEDIUM REMOVAL"
static atomic_uint s_writes_ok;

static atomic_bool s_eject_seen;

#define BEAMER_MSC_UNSUP 8

static uint8_t s_unsup_op[BEAMER_MSC_UNSUP];
static atomic_uint s_unsup_count[BEAMER_MSC_UNSUP];
static atomic_uint s_unsup_used;
static int64_t s_last_cbw_us;
static atomic_uint s_maxlun;

static uint32_t s_visible;

static uint32_t visible_sectors(void)
{
    if (s_visible != 0)
    {
        return s_visible;
    }
    return s_card != NULL ? (uint32_t)s_card->csd.capacity : 0;
}

void beamer_msc_set_visible(uint32_t sectors)
{
    s_visible = sectors;
}

#define BEAMER_MSC_CENSUS 16

static uint8_t s_cen_op[BEAMER_MSC_CENSUS];
static atomic_uint s_cen_count[BEAMER_MSC_CENSUS];
static atomic_uint s_cen_used;

static void census(uint8_t op)
{
    const unsigned used = atomic_load(&s_cen_used);
    for (unsigned i = 0; i < used && i < BEAMER_MSC_CENSUS; i++)
    {
        if (s_cen_op[i] == op)
        {
            atomic_fetch_add(&s_cen_count[i], 1);
            return;
        }
    }
    if (used >= BEAMER_MSC_CENSUS)
    {
        return;
    }
    s_cen_op[used] = op;
    atomic_store(&s_cen_count[used], 1);
    atomic_store(&s_cen_used, used + 1);
}

void tud_msc_scsi_complete_cb(uint8_t lun, uint8_t const scsi_cmd[16])
{
    (void)lun;
    census(scsi_cmd[0]);
}

#define BEAMER_MSC_RING 512 /* 512 * 16 B = 8 KB of internal SRAM. */

static beamer_msc_sample_t s_ring[BEAMER_MSC_RING];
static atomic_uint s_head;
static atomic_uint s_tail;
static atomic_uint s_dropped;
static portMUX_TYPE s_ring_mux = portMUX_INITIALIZER_UNLOCKED;

void beamer_msc_ring_push(uint8_t op, uint32_t lba, uint16_t blocks, int64_t t0, int64_t t1,
                          esp_err_t err)
{
    const beamer_msc_sample_t sample = {
        .start_us = (uint32_t)t0,
        .dur_us = (uint32_t)(t1 - t0),
        .lba = lba,
        .blocks = blocks,
        .op = op,
        .err = (uint8_t)(err == ESP_OK ? 0 : (err & 0xff)),
    };

    portENTER_CRITICAL_SAFE(&s_ring_mux);
    const unsigned head = atomic_load_explicit(&s_head, memory_order_relaxed);
    const unsigned tail = atomic_load_explicit(&s_tail, memory_order_acquire);
    if (head - tail >= BEAMER_MSC_RING)
    {
        atomic_fetch_add_explicit(&s_dropped, 1, memory_order_relaxed);
        portEXIT_CRITICAL_SAFE(&s_ring_mux);
        return;
    }
    s_ring[head % BEAMER_MSC_RING] = sample;
    atomic_store_explicit(&s_head, head + 1, memory_order_release);
    portEXIT_CRITICAL_SAFE(&s_ring_mux);
}

size_t beamer_msc_drain(beamer_msc_sample_t *out, size_t max)
{
    const unsigned head = atomic_load_explicit(&s_head, memory_order_acquire);
    unsigned tail = atomic_load_explicit(&s_tail, memory_order_relaxed);

    size_t n = 0;
    while (tail != head && n < max)
    {
        out[n++] = s_ring[tail % BEAMER_MSC_RING];
        tail++;
    }
    atomic_store_explicit(&s_tail, tail, memory_order_release);
    return n;
}

uint32_t beamer_msc_dropped(void)
{
    return atomic_load(&s_dropped);
}

static int32_t transfer(bool write, uint32_t lba, uint32_t offset, void *buffer, uint32_t bufsize)
{
    if (!atomic_load(&s_media_present) || s_card == NULL)
    {
        return -1;
    }

    const uint32_t ssz = s_card->csd.sector_size;
    if (ssz == 0 || (offset % ssz) != 0 || (bufsize % ssz) != 0)
    {
        return -1;
    }

    const uint32_t start = lba + offset / ssz;
    const size_t count = bufsize / ssz;
    if (count == 0)
    {
        return 0;
    }

    const uint32_t limit = visible_sectors();
    if (limit == 0 || start >= limit || (uint32_t)count > limit - start)
    {
        const int64_t t = esp_timer_get_time();
        beamer_msc_ring_push(write ? BEAMER_MSC_OP_WRITE : BEAMER_MSC_OP_READ, start,
                             (uint16_t)count, t, t, ESP_ERR_INVALID_SIZE);
        int none = 0;
        if (atomic_compare_exchange_strong(&s_first_err, &none, (int)ESP_ERR_INVALID_SIZE))
        {
            ESP_EARLY_LOGE(TAG, "%s lba %u x%u past visible %u", write ? "write" : "read",
                           (unsigned)start, (unsigned)count, (unsigned)limit);
        }
        return -1;
    }

    const int64_t t0 = esp_timer_get_time();
    s_last_cbw_us = t0;
    const esp_err_t err = write ? beamer_wbc_write(start, buffer, count)
                                : beamer_wbc_read(start, buffer, count);
    const int64_t t1 = esp_timer_get_time();

    beamer_msc_ring_push(write ? BEAMER_MSC_OP_WRITE : BEAMER_MSC_OP_READ, start, (uint16_t)count,
                         t0, t1, err);

    if (err != ESP_OK)
    {
        int none = 0;
        if (atomic_compare_exchange_strong(&s_first_err, &none, (int)err))
        {
            ESP_EARLY_LOGE(TAG, "%s lba %u x%u failed: 0x%x", write ? "write" : "read",
                           (unsigned)start, (unsigned)count, (unsigned)err);
        }
        return -1;
    }
    if (write)
    {
        atomic_store(&s_dirty, true);
        atomic_fetch_add(&s_writes_ok, count);
    }
    else
    {
        atomic_fetch_add(&s_reads_ok, count);
    }
    return (int32_t)bufsize;
}

int32_t tud_msc_read10_cb(uint8_t lun, uint32_t lba, uint32_t offset, void *buffer, uint32_t bufsize)
{
    (void)lun;
    return transfer(false, lba, offset, buffer, bufsize);
}

int32_t tud_msc_write10_cb(uint8_t lun, uint32_t lba, uint32_t offset, uint8_t *buffer, uint32_t bufsize)
{
    (void)lun;
    return transfer(true, lba, offset, buffer, bufsize);
}

uint8_t tud_msc_get_maxlun_cb(void)
{
    atomic_fetch_add(&s_maxlun, 1);
    return 1;
}

void tud_msc_inquiry_cb(uint8_t lun, uint8_t vendor_id[8], uint8_t product_id[16], uint8_t product_rev[4])
{
    (void)lun;
    memcpy(vendor_id, "SanDisk ", 8);
    memcpy(product_id, "Cruzer Blade    ", 16);
    memcpy(product_rev, "1.00", 4);
}

bool tud_msc_test_unit_ready_cb(uint8_t lun)
{
    if (!atomic_load(&s_media_present))
    {
        tud_msc_set_sense(lun, SCSI_SENSE_NOT_READY, 0x3A, 0x00);
        return false;
    }
    return true;
}

void tud_msc_capacity_cb(uint8_t lun, uint32_t *block_count, uint16_t *block_size)
{
    (void)lun;
    if (s_card == NULL)
    {
        *block_count = 0;
        *block_size = 0;
        return;
    }
    *block_count = visible_sectors();
    *block_size = (uint16_t)s_card->csd.sector_size;
}

bool tud_msc_is_writable_cb(uint8_t lun)
{
    (void)lun;
    return atomic_load(&s_media_present) && beamer_wbc_policy() != BEAMER_WBC_REFUSE;
}

bool tud_msc_start_stop_cb(uint8_t lun, uint8_t power_condition, bool start, bool load_eject)
{
    (void)lun;
    (void)power_condition;
    if (!load_eject)
    {
        return true;
    }

    if (!start)
    {
        atomic_store(&s_eject, true);
        atomic_store(&s_eject_seen, true);
        ESP_EARLY_LOGI(TAG, "host ejected");
        return true;
    }

    atomic_store(&s_media_present, true);
    atomic_store(&s_load, true);
    ESP_EARLY_LOGI(TAG, "host loaded");
    return true;
}

bool tud_msc_prevent_allow_medium_removal_cb(uint8_t lun, uint8_t prohibit_removal, uint8_t control)
{
    (void)lun;
    (void)control;
    atomic_store(&s_locked, prohibit_removal != 0);
    return true;
}

static void note_unsupported(uint8_t op)
{
    const unsigned used = atomic_load(&s_unsup_used);
    for (unsigned i = 0; i < used && i < BEAMER_MSC_UNSUP; i++)
    {
        if (s_unsup_op[i] == op)
        {
            atomic_fetch_add(&s_unsup_count[i], 1);
            return;
        }
    }
    if (used >= BEAMER_MSC_UNSUP)
    {
        return;
    }
    s_unsup_op[used] = op;
    atomic_store(&s_unsup_count[used], 1);
    atomic_store(&s_unsup_used, used + 1);
    ESP_EARLY_LOGW(TAG, "unsupported SCSI opcode 0x%x", (unsigned)op);
}

int32_t tud_msc_scsi_cb(uint8_t lun, uint8_t const scsi_cmd[16], void *buffer, uint16_t bufsize)
{
    (void)buffer;
    (void)bufsize;
    switch (scsi_cmd[0])
    {
    case BEAMER_SCSI_CMD_SYNCHRONIZE_CACHE_10:
        if (beamer_wbc_flush_all() != ESP_OK)
        {
            tud_msc_set_sense(lun, SCSI_SENSE_MEDIUM_ERROR, 0x0C, 0x00);
            return -1;
        }
        return 0;
    default:
        note_unsupported(scsi_cmd[0]);
        tud_msc_set_sense(lun, SCSI_SENSE_ILLEGAL_REQUEST, 0x20, 0x00);
        return -1;
    }
}

static atomic_uint s_mounts;
static atomic_uint s_umounts;

void tud_mount_cb(void)
{
    atomic_fetch_add(&s_mounts, 1);
    ESP_EARLY_LOGI(TAG, "host configured us");
}

void tud_umount_cb(void)
{
    atomic_fetch_add(&s_umounts, 1);
    atomic_store(&s_locked, false);
    ESP_EARLY_LOGI(TAG, "host dropped us");
}

bool beamer_msc_mounted(void)
{
    return tud_mounted();
}

uint32_t beamer_msc_mounts(void)
{
    return atomic_load(&s_mounts);
}

uint32_t beamer_msc_umounts(void)
{
    return atomic_load(&s_umounts);
}

uint32_t beamer_msc_reads_ok(void)
{
    return atomic_load(&s_reads_ok);
}

uint32_t beamer_msc_writes_ok(void)
{
    return atomic_load(&s_writes_ok);
}

bool beamer_msc_eject_seen(void)
{
    return atomic_load(&s_eject_seen);
}

uint32_t beamer_msc_maxlun_asks(void)
{
    return atomic_load(&s_maxlun);
}

int64_t beamer_msc_last_cbw_us(void)
{
    return s_last_cbw_us;
}

size_t beamer_msc_census(beamer_msc_unsup_t *out, size_t max)
{
    if (out == NULL)
    {
        return 0;
    }
    const unsigned used = atomic_load(&s_cen_used);
    size_t n = 0;
    for (unsigned i = 0; i < used && i < BEAMER_MSC_CENSUS && n < max; i++)
    {
        out[n].op = s_cen_op[i];
        out[n].count = atomic_load(&s_cen_count[i]);
        n++;
    }
    return n;
}

size_t beamer_msc_unsupported(beamer_msc_unsup_t *out, size_t max)
{
    if (out == NULL)
    {
        return 0;
    }
    const unsigned used = atomic_load(&s_unsup_used);
    size_t n = 0;
    for (unsigned i = 0; i < used && i < BEAMER_MSC_UNSUP && n < max; i++)
    {
        out[n].op = s_unsup_op[i];
        out[n].count = atomic_load(&s_unsup_count[i]);
        n++;
    }
    return n;
}

int beamer_msc_first_err(void)
{
    return atomic_load(&s_first_err);
}

bool beamer_msc_host_owns(void)
{
    return atomic_load(&s_locked);
}

bool beamer_msc_suspended(void)
{
    return tud_suspended();
}

bool beamer_msc_take_dirty(void)
{
    return atomic_exchange(&s_dirty, false);
}

bool beamer_msc_take_eject(void)
{
    return atomic_exchange(&s_eject, false);
}

bool beamer_msc_take_load(void)
{
    return atomic_exchange(&s_load, false);
}

void beamer_msc_set_media(bool present)
{
    atomic_store(&s_media_present, present);
    ESP_LOGI(TAG, "medium %s", present ? "present" : "withdrawn");
}

bool beamer_msc_media_present(void)
{
    return atomic_load(&s_media_present);
}

void beamer_msc_detach(void)
{
    tud_disconnect();
    ESP_LOGI(TAG, "detached from the bus");
}

int64_t beamer_msc_bind_time_us(void)
{
    return s_bind_us;
}

static struct
{
    SemaphoreHandle_t done;
    esp_err_t err;
} s_init;

static void beamer_msc_task(void *arg)
{
    (void)arg;

    usb_phy_handle_t phy = NULL;
    usb_phy_config_t phy_cfg = {
        .controller = USB_PHY_CTRL_OTG,
        .target = USB_PHY_TARGET_INT,
        .otg_mode = USB_OTG_MODE_DEVICE,
        .otg_speed = USB_PHY_SPEED_FULL,
    };

    esp_err_t err = usb_new_phy(&phy_cfg, &phy);
    if (err == ESP_OK)
    {
        atomic_store(&s_media_present, true);
        s_bind_us = esp_timer_get_time();

        const tusb_rhport_init_t rhport = {
            .role = TUSB_ROLE_DEVICE,
            .speed = TUSB_SPEED_FULL,
        };
        if (!tusb_rhport_init(0, &rhport))
        {
            err = ESP_FAIL;
        }
    }

    s_init.err = err;
    xSemaphoreGive(s_init.done);

    if (err != ESP_OK)
    {
        vTaskDelete(NULL);
    }

    while (1) // this should only ever cal tud_task() - putting other things makes it too slow
    {
        tud_task();
    }
}

esp_err_t beamer_msc_install(sdmmc_card_t *card, SemaphoreHandle_t lock, const char *serial)
{
    ESP_RETURN_ON_FALSE(card && lock && serial, ESP_ERR_INVALID_ARG, TAG, "null argument");

    s_card = card;
    s_lock = lock;
    strlcpy(s_serial, serial, sizeof(s_serial));
    ESP_RETURN_ON_ERROR(beamer_wbc_start(card, lock), TAG, "write-back cache");

    s_init.done = xSemaphoreCreateBinary();
    ESP_RETURN_ON_FALSE(s_init.done, ESP_ERR_NO_MEM, TAG, "init semaphore");
    s_init.err = ESP_FAIL;

    BaseType_t ok = xTaskCreatePinnedToCore(beamer_msc_task, "beamer_msc", 6144, NULL, 22, NULL, 1);
    ESP_RETURN_ON_FALSE(ok == pdPASS, ESP_ERR_NO_MEM, TAG, "task create");

    xSemaphoreTake(s_init.done, portMAX_DELAY);
    ESP_RETURN_ON_ERROR(s_init.err, TAG, "tinyusb bring-up");

    ESP_LOGI(TAG, "bound as %s", s_serial);
    return ESP_OK;
}

#include "driver/sdmmc_host.h"

#define BEAMER_SD_CLK 12
#define BEAMER_SD_CMD 16
#define BEAMER_SD_D0 14
#define BEAMER_SD_D1 17
#define BEAMER_SD_D2 21
#define BEAMER_SD_D3 18

#define BEAMER_SD_FREQ_KHZ SDMMC_FREQ_DEFAULT

esp_err_t beamer_sd_init(sdmmc_card_t **out_card, SemaphoreHandle_t *out_lock)
{
    ESP_RETURN_ON_FALSE(out_card && out_lock, ESP_ERR_INVALID_ARG, TAG, "null argument");

    sdmmc_host_t host = SDMMC_HOST_DEFAULT();
    host.slot = SDMMC_HOST_SLOT_1;
    host.flags = SDMMC_HOST_FLAG_4BIT | SDMMC_HOST_FLAG_1BIT;
    host.max_freq_khz = BEAMER_SD_FREQ_KHZ;

    sdmmc_slot_config_t slot = SDMMC_SLOT_CONFIG_DEFAULT();
    slot.width = 4;
    slot.clk = BEAMER_SD_CLK;
    slot.cmd = BEAMER_SD_CMD;
    slot.d0 = BEAMER_SD_D0;
    slot.d1 = BEAMER_SD_D1;
    slot.d2 = BEAMER_SD_D2;
    slot.d3 = BEAMER_SD_D3;
    slot.flags |= SDMMC_SLOT_FLAG_INTERNAL_PULLUP;

    ESP_RETURN_ON_ERROR(host.init(), TAG, "sdmmc_host_init");

    esp_err_t err = sdmmc_host_init_slot(host.slot, &slot);
    if (err != ESP_OK)
    {
        ESP_LOGE(TAG, "sdmmc_host_init_slot: 0x%x", err);
        host.deinit();
        return err;
    }

    sdmmc_card_t *card = calloc(1, sizeof(sdmmc_card_t));
    if (card == NULL)
    {
        host.deinit();
        return ESP_ERR_NO_MEM;
    }

    err = sdmmc_card_init(&host, card);
    if (err != ESP_OK)
    {
        free(card);
        host.deinit();
        return err;
    }
    ESP_LOGI(TAG, "card probed at %d kHz, %d-bit", host.max_freq_khz,
             (int)sdmmc_host_get_slot_width(host.slot));

    SemaphoreHandle_t lock = xSemaphoreCreateMutex();
    if (lock == NULL)
    {
        free(card);
        host.deinit();
        return ESP_ERR_NO_MEM;
    }

    *out_card = card;
    *out_lock = lock;
    return ESP_OK;
}

esp_err_t beamer_sd_probe_partition(sdmmc_card_t *card, SemaphoreHandle_t lock,
                                    uint32_t max_sectors, beamer_part_t *out)
{
    ESP_RETURN_ON_FALSE(card && lock && out, ESP_ERR_INVALID_ARG, TAG, "null argument");

    static uint8_t mbr[512] __attribute__((aligned(4)));

    *out = (beamer_part_t){0};

    if (xSemaphoreTake(lock, pdMS_TO_TICKS(5000)) != pdTRUE)
    {
        return ESP_ERR_TIMEOUT;
    }
    const esp_err_t err = sdmmc_read_sectors(card, mbr, 0, 1);
    xSemaphoreGive(lock);
    if (err != ESP_OK)
    {
        return err;
    }

    if (mbr[510] != 0x55 || mbr[511] != 0xAA)
    {
        return ESP_ERR_NOT_FOUND;
    }

    for (int i = 0; i < 4; i++)
    {
        const uint8_t *e = &mbr[446 + 16 * i];
        const uint8_t type = e[4];
        if (type == 0x00)
        {
            continue;
        }
        if (type != 0x0B && type != 0x0C)
        {
            continue;
        }
        out->type = type;
        out->start = (uint32_t)e[8] | ((uint32_t)e[9] << 8) | ((uint32_t)e[10] << 16) |
                     ((uint32_t)e[11] << 24);
        out->sectors = (uint32_t)e[12] | ((uint32_t)e[13] << 8) | ((uint32_t)e[14] << 16) |
                       ((uint32_t)e[15] << 24);
        return out->sectors > max_sectors ? ESP_ERR_INVALID_SIZE : ESP_OK;
    }

    return ESP_ERR_NOT_FOUND;
}

uint64_t beamer_sd_bytes(const sdmmc_card_t *card)
{
    if (card == NULL)
    {
        return 0;
    }
    return (uint64_t)card->csd.capacity * (uint64_t)card->csd.sector_size;
}
