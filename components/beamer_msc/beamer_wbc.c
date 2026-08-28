/*
 * The write-back cache: 32 KB of internal SRAM between the host and the card
 * prevents SD card stalls (common!) from being visible to the host.
 *
 * Three things to note:
 *
 * 1. always flush in insertion order (wbc_slot_t.seq)
 * 2. reads consult this cache
 * 3. on full cache, block - synchronous fallback defeats the purpose of caching!
 *
 */

#include "beamer_msc.h"

#include <stdatomic.h>
#include <string.h>

#include "esp_log.h"
#include "esp_timer.h"
#include "freertos/task.h"

static const char *TAG = "beamer_wbc";

#define WBC_SECTORS 64
#define WBC_SECTOR_SZ 512
#define WBC_FLUSH_RUN 16

#define WBC_STALL_SLICE_MS 50
#define WBC_STALL_SLICES 100

static __attribute__((aligned(4))) uint8_t s_data[WBC_SECTORS][WBC_SECTOR_SZ];

typedef struct
{
    uint32_t lba;
    uint32_t seq;
    bool valid;
    bool dirty;
} wbc_slot_t;

static wbc_slot_t s_meta[WBC_SECTORS];

static sdmmc_card_t *s_card;
static SemaphoreHandle_t s_lock;
static SemaphoreHandle_t s_meta_lock;

static SemaphoreHandle_t s_flush_lock;
static __attribute__((aligned(4))) uint8_t s_staging[WBC_FLUSH_RUN * WBC_SECTOR_SZ];

static uint32_t s_seq;
static atomic_uint s_dirty;
static atomic_uint s_high_water;
static atomic_uint s_stalls;
static atomic_int s_policy = BEAMER_WBC_WRITEBACK;

static SemaphoreHandle_t s_work; // wakes flush
static SemaphoreHandle_t s_room; // wakes "out of sloots" writer

static int find(uint32_t lba)
{
    for (int i = 0; i < WBC_SECTORS; i++)
    {
        if (s_meta[i].valid && s_meta[i].lba == lba)
        {
            return i;
        }
    }
    return -1;
}

static int find_free(void)
{
    for (int i = 0; i < WBC_SECTORS; i++)
    {
        if (!s_meta[i].valid)
        {
            return i;
        }
    }
    return -1;
}

static int oldest_dirty(void)
{
    int best = -1;
    for (int i = 0; i < WBC_SECTORS; i++)
    {
        if (s_meta[i].dirty && (best < 0 || s_meta[i].seq < s_meta[best].seq))
        {
            best = i;
        }
    }
    return best;
}

static int oldest_clean(void)
{
    int best = -1;
    for (int i = 0; i < WBC_SECTORS; i++)
    {
        if (s_meta[i].valid && !s_meta[i].dirty && (best < 0 || s_meta[i].seq < s_meta[best].seq))
        {
            best = i;
        }
    }
    return best;
}

static void mark_dirty(int slot)
{
    if (!s_meta[slot].dirty)
    {
        s_meta[slot].dirty = true;
        const unsigned n = atomic_fetch_add(&s_dirty, 1) + 1;
        if (n > atomic_load(&s_high_water))
        {
            atomic_store(&s_high_water, n);
        }
    }
}

static void mark_clean(int slot)
{
    if (s_meta[slot].dirty)
    {
        s_meta[slot].dirty = false;
        atomic_fetch_sub(&s_dirty, 1);
    }
}

static esp_err_t flush_one_run(void)
{
    int slots[WBC_FLUSH_RUN];
    uint32_t seqs[WBC_FLUSH_RUN];
    uint32_t start_lba;
    size_t n = 0;

    xSemaphoreTake(s_flush_lock, portMAX_DELAY);

    xSemaphoreTake(s_meta_lock, portMAX_DELAY);
    const int first = oldest_dirty();
    if (first < 0)
    {
        xSemaphoreGive(s_meta_lock);
        xSemaphoreGive(s_flush_lock);
        return ESP_OK;
    }
    start_lba = s_meta[first].lba;
    uint32_t seq = s_meta[first].seq;

    while (n < WBC_FLUSH_RUN)
    {
        const int slot = find(start_lba + n);
        if (slot < 0 || !s_meta[slot].dirty || s_meta[slot].seq < seq)
        {
            break;
        }
        seq = s_meta[slot].seq;
        memcpy(s_staging + n * WBC_SECTOR_SZ, s_data[slot], WBC_SECTOR_SZ);
        slots[n] = slot;
        seqs[n] = s_meta[slot].seq;
        n++;
    }
    xSemaphoreGive(s_meta_lock);

    if (n == 0)
    {
        xSemaphoreGive(s_flush_lock);
        return ESP_OK;
    }

    const int64_t t0 = esp_timer_get_time();
    if (xSemaphoreTake(s_lock, pdMS_TO_TICKS(5000)) != pdTRUE)
    {
        xSemaphoreGive(s_flush_lock);
        return ESP_ERR_TIMEOUT;
    }
    const esp_err_t err = sdmmc_write_sectors(s_card, s_staging, start_lba, n);
    xSemaphoreGive(s_lock);
    const int64_t t1 = esp_timer_get_time();

    beamer_msc_ring_push(BEAMER_MSC_OP_FLUSH, start_lba, (uint16_t)n, t0, t1, err);

    xSemaphoreTake(s_meta_lock, portMAX_DELAY);
    for (size_t i = 0; i < n; i++)
    {
        if (err == ESP_OK && s_meta[slots[i]].seq == seqs[i])
        {
            mark_clean(slots[i]);
        }
    }
    xSemaphoreGive(s_meta_lock);
    xSemaphoreGive(s_flush_lock);

    xSemaphoreGive(s_room);
    return err;
}

static void wbc_flush_task(void *arg)
{
    (void)arg;
    while (1)
    {
        xSemaphoreTake(s_work, pdMS_TO_TICKS(50));
        while (atomic_load(&s_dirty) > 0)
        {
            if (flush_one_run() != ESP_OK)
            {
                vTaskDelay(pdMS_TO_TICKS(20));
                break;
            }
        }
    }
}

esp_err_t beamer_wbc_start(sdmmc_card_t *card, SemaphoreHandle_t lock)
{
    if (card == NULL || lock == NULL)
    {
        return ESP_ERR_INVALID_ARG;
    }
    s_card = card;
    s_lock = lock;

    s_meta_lock = xSemaphoreCreateMutex();
    s_flush_lock = xSemaphoreCreateMutex();
    s_work = xSemaphoreCreateBinary();
    s_room = xSemaphoreCreateBinary();
    if (s_meta_lock == NULL || s_flush_lock == NULL || s_work == NULL || s_room == NULL)
    {
        return ESP_ERR_NO_MEM;
    }

    const BaseType_t ok =
        xTaskCreatePinnedToCore(wbc_flush_task, "beamer_wbc", 4096, NULL, 10, NULL, 1);
    if (ok != pdPASS)
    {
        return ESP_ERR_NO_MEM;
    }
    ESP_LOGI(TAG, "write-back cache: %d sectors, %d KB", WBC_SECTORS,
             (WBC_SECTORS * WBC_SECTOR_SZ) / 1024);
    return ESP_OK;
}

static esp_err_t write_direct(uint32_t lba, const void *buf, size_t count)
{
    const int64_t t0 = esp_timer_get_time();
    if (xSemaphoreTake(s_lock, pdMS_TO_TICKS(5000)) != pdTRUE)
    {
        return ESP_ERR_TIMEOUT;
    }
    const esp_err_t err = sdmmc_write_sectors(s_card, buf, lba, count);
    xSemaphoreGive(s_lock);
    const int64_t t1 = esp_timer_get_time();
    beamer_msc_ring_push(BEAMER_MSC_OP_FLUSH, lba, (uint16_t)count, t0, t1, err);
    return err;
}

esp_err_t beamer_wbc_write(uint32_t lba, const void *buf, size_t count)
{
    const beamer_wbc_policy_t policy = atomic_load(&s_policy);

    if (policy == BEAMER_WBC_REFUSE)
    {
        return ESP_ERR_NOT_SUPPORTED;
    }
    if (policy == BEAMER_WBC_WRITETHROUGH)
    {
        xSemaphoreTake(s_meta_lock, portMAX_DELAY);
        for (size_t i = 0; i < count; i++)
        {
            const int slot = find(lba + i);
            if (slot >= 0)
            {
                mark_clean(slot);
                s_meta[slot].valid = false;
            }
        }
        xSemaphoreGive(s_meta_lock);
        return write_direct(lba, buf, count);
    }

    const uint8_t *src = (const uint8_t *)buf;
    size_t i = 0;
    while (i < count)
    {
        const uint32_t want = lba + i;

        xSemaphoreTake(s_meta_lock, portMAX_DELAY);
        int slot = find(want);
        if (slot >= 0)
        {
            memcpy(s_data[slot], src + i * WBC_SECTOR_SZ, WBC_SECTOR_SZ);
            mark_dirty(slot);
            xSemaphoreGive(s_meta_lock);
            xSemaphoreGive(s_work);
            i++;
            continue;
        }

        slot = find_free();
        if (slot < 0)
        {
            slot = oldest_clean();
        }
        if (slot < 0) // full: block until theres another slot
        {
            xSemaphoreGive(s_meta_lock);
            atomic_fetch_add(&s_stalls, 1);
            xSemaphoreGive(s_work);

            bool room = false;
            for (int i = 0; i < WBC_STALL_SLICES && !room; i++)
            {
                xSemaphoreTake(s_room, pdMS_TO_TICKS(WBC_STALL_SLICE_MS));
                xSemaphoreTake(s_meta_lock, portMAX_DELAY);
                room = find_free() >= 0 || oldest_clean() >= 0;
                xSemaphoreGive(s_meta_lock);
            }
            if (!room)
            {
                return ESP_ERR_TIMEOUT;
            }
            continue;
        }

        memcpy(s_data[slot], src + i * WBC_SECTOR_SZ, WBC_SECTOR_SZ);
        s_meta[slot] = (wbc_slot_t){
            .lba = want,
            .seq = ++s_seq,
            .valid = true,
            .dirty = false,
        };
        mark_dirty(slot);
        xSemaphoreGive(s_meta_lock);
        xSemaphoreGive(s_work);
        i++;
    }
    return ESP_OK;
}

esp_err_t beamer_wbc_read(uint32_t lba, void *buf, size_t count)
{
    uint8_t *dst = (uint8_t *)buf;
    size_t i = 0;

    while (i < count)
    {
        xSemaphoreTake(s_meta_lock, portMAX_DELAY);
        const int slot = find(lba + i);
        if (slot >= 0)
        {
            memcpy(dst + i * WBC_SECTOR_SZ, s_data[slot], WBC_SECTOR_SZ);
            xSemaphoreGive(s_meta_lock);
            i++;
            continue;
        }
        size_t run = 0;
        while (i + run < count && find(lba + i + run) < 0)
        {
            run++;
        }
        xSemaphoreGive(s_meta_lock);

        if (xSemaphoreTake(s_lock, pdMS_TO_TICKS(5000)) != pdTRUE)
        {
            return ESP_ERR_TIMEOUT;
        }
        const esp_err_t err =
            sdmmc_read_sectors(s_card, dst + i * WBC_SECTOR_SZ, lba + i, run);
        xSemaphoreGive(s_lock);
        if (err != ESP_OK)
        {
            return err;
        }
        i += run;
    }
    return ESP_OK;
}

esp_err_t beamer_wbc_flush_all(void)
{
    if (s_flush_lock == NULL)
    {
        return ESP_OK;
    }

    while (atomic_load(&s_dirty) > 0)
    {
        const esp_err_t err = flush_one_run();
        if (err != ESP_OK)
        {
            return err;
        }
    }
    return ESP_OK;
}

void beamer_wbc_set_policy(beamer_wbc_policy_t policy)
{
    atomic_store(&s_policy, policy);
    if (s_meta_lock == NULL)
    {
        return;
    }

    (void)beamer_wbc_flush_all();

    if (policy != BEAMER_WBC_WRITEBACK)
    {
        xSemaphoreTake(s_meta_lock, portMAX_DELAY);
        for (int i = 0; i < WBC_SECTORS; i++)
        {
            mark_clean(i);
            s_meta[i].valid = false;
        }
        xSemaphoreGive(s_meta_lock);
    }
    ESP_LOGI(TAG, "policy now %d", (int)policy);
}

beamer_wbc_policy_t beamer_wbc_policy(void)
{
    return (beamer_wbc_policy_t)atomic_load(&s_policy);
}

uint32_t beamer_wbc_dirty(void)
{
    return atomic_load(&s_dirty);
}

uint32_t beamer_wbc_capacity(void)
{
    return WBC_SECTORS;
}

uint32_t beamer_wbc_high_water(void)
{
    return atomic_load(&s_high_water);
}

uint32_t beamer_wbc_stalls(void)
{
    return atomic_load(&s_stalls);
}
