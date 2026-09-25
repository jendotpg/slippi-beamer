/*
 * The write-back cache: 16 KB of internal SRAM between the host and the card
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

#include <inttypes.h>
#include <stdatomic.h>
#include <string.h>

#include "esp_log.h"
#include "esp_timer.h"
#include "freertos/task.h"

static const char *TAG = "beamer_wbc";

#define WBC_SECTORS 32
#define WBC_SECTOR_SZ 512
#define WBC_FLUSH_RUN 16

#define WBC_STALL_SLICE_MS 50

#define WBC_REINIT_AFTER 5                // failed card commands in a row (500 ms each, beamer_msc.c)
#define WBC_REINIT_GAP_US (1000 * 1000)   // between re-init attempts
#define WBC_BUSY_TIMEOUT_US (5000 * 1000) // post-write, before busy must be gone

// only touch with s_meta_lock held
static __attribute__((aligned(4))) uint8_t s_data[WBC_SECTORS][WBC_SECTOR_SZ];

typedef struct
{
    uint32_t lba;
    uint32_t seq;
    bool valid;
    bool dirty;
} wbc_slot_t;

static wbc_slot_t s_meta[WBC_SECTORS]; // only touch with s_meta_lock held

static sdmmc_card_t *s_card; // only send commands to (or re-init) the card with s_lock held
static SemaphoreHandle_t s_lock;
static SemaphoreHandle_t s_meta_lock;

static SemaphoreHandle_t s_flush_lock;
static __attribute__((aligned(4))) uint8_t s_staging[WBC_FLUSH_RUN * WBC_SECTOR_SZ];

static uint32_t s_seq; // only touch with s_meta_lock held
static atomic_uint s_dirty;
static atomic_uint s_high_water;
static atomic_uint s_stalls;
static atomic_int s_policy = BEAMER_WBC_WRITEBACK;
static atomic_uint s_read_wait_us;
static atomic_uint s_read_wait_max_us;

// only touch with s_lock held
static unsigned s_fail_streak;
static bool s_card_lost;
static int64_t s_last_reinit_us;
static sdmmc_card_t s_fresh; // re-init lands here first, so a failed one leaves s_card alone

static atomic_uint s_write_failures;
static atomic_uint s_busy_timeouts;
static atomic_bool s_writes_healthy = true;

static SemaphoreHandle_t s_work; // wakes flush
static SemaphoreHandle_t s_room; // wakes "out of sloots" writer

#define WBC_STACK 4096

static StaticSemaphore_t s_meta_lock_buf;
static StaticSemaphore_t s_flush_lock_buf;
static StaticSemaphore_t s_work_buf;
static StaticSemaphore_t s_room_buf;
static StackType_t s_stack[WBC_STACK];
static StaticTask_t s_tcb;

// only call with s_meta_lock held
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

// only call with s_meta_lock held
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

// only call with s_meta_lock held
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

// only call with s_meta_lock held
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

// only call with s_meta_lock held
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

// only call with s_meta_lock held
static void mark_clean(int slot)
{
    if (s_meta[slot].dirty)
    {
        s_meta[slot].dirty = false;
        atomic_fetch_sub(&s_dirty, 1);
    }
}

// only call with s_lock held
static void reinit_locked(const char *why)
{
    const int64_t now = esp_timer_get_time();
    s_last_reinit_us = now;
    s_fail_streak = 0;

    if (!s_card_lost)
    {
        sdmmc_command_t probe = {
            .opcode = MMC_SEND_STATUS,
            .arg = MMC_ARG_RCA(s_card->rca),
            .flags = SCF_CMD_AC | SCF_RSP_R1,
            .timeout_ms = s_card->host.command_timeout_ms,
        };
        const esp_err_t perr = s_card->host.do_transaction(s_card->host.slot, &probe);
        if (perr == ESP_OK)
        {
            ESP_LOGW(TAG, "%s: card stalled, answers status in state %u (r1 0x%08" PRIx32 ")",
                     why, (unsigned)MMC_R1_CURRENT_STATE(probe.response), probe.response[0]);
        }
        else
        {
            ESP_LOGW(TAG, "%s: card lost, no answer to status at its address (0x%x)", why, perr);
        }
    }

    s_card->host.set_bus_width(s_card->host.slot, 1);
    s_card->host.set_card_clk(s_card->host.slot, SDMMC_FREQ_PROBING);

    const esp_err_t rc = sdmmc_card_init(&s_card->host, &s_fresh);
    const int64_t took = esp_timer_get_time() - now;

    s_card_lost = rc != ESP_OK;
    if (rc == ESP_OK)
    {
        *s_card = s_fresh;
        atomic_store(&s_writes_healthy, true);
        ESP_LOGW(TAG, "%s: re-init ok in %lld us", why, took);
    }
    else
    {
        ESP_LOGE(TAG, "%s: re-init failed: %s (0x%x) after %lld us", why, esp_err_to_name(rc), rc,
                 took);
    }
}

// only call with s_lock held
static void card_result_locked(esp_err_t err, bool busy)
{
    if (err == ESP_OK)
    {
        s_fail_streak = 0;
        return;
    }
    if (!s_card_lost && (err == ESP_ERR_INVALID_SIZE || err == ESP_ERR_INVALID_ARG))
    {
        return;
    }
    if (busy)
    {
        reinit_locked("card stayed busy after a write");
        return;
    }
    if (++s_fail_streak < WBC_REINIT_AFTER)
    {
        return;
    }
    const int64_t now = esp_timer_get_time();
    if (s_last_reinit_us != 0 && now - s_last_reinit_us < WBC_REINIT_GAP_US)
    {
        return;
    }
    reinit_locked("card commands failing");
}

// only call with s_lock held
static esp_err_t card_write_locked(uint32_t lba, const void *buf, size_t count)
{
    const int64_t t0 = esp_timer_get_time();
    const esp_err_t err = sdmmc_write_sectors(s_card, buf, lba, count);
    const bool busy =
        err == ESP_ERR_TIMEOUT && esp_timer_get_time() - t0 >= WBC_BUSY_TIMEOUT_US;
    if (err == ESP_OK)
    {
        atomic_store(&s_writes_healthy, true);
    }
    else
    {
        atomic_store(&s_writes_healthy, false);
        atomic_fetch_add(busy ? &s_busy_timeouts : &s_write_failures, 1);
    }
    card_result_locked(err, busy);
    return err;
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
    const esp_err_t err = card_write_locked(start_lba, s_staging, n);
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

    s_meta_lock = xSemaphoreCreateMutexStatic(&s_meta_lock_buf);
    s_flush_lock = xSemaphoreCreateMutexStatic(&s_flush_lock_buf);
    s_work = xSemaphoreCreateBinaryStatic(&s_work_buf);
    s_room = xSemaphoreCreateBinaryStatic(&s_room_buf);

    xTaskCreateStaticPinnedToCore(wbc_flush_task, "beamer_wbc", WBC_STACK, NULL, 10, s_stack,
                                  &s_tcb, 1);
    ESP_LOGI(TAG, "write-back cache: %d sectors, %d KB", WBC_SECTORS,
             (WBC_SECTORS * WBC_SECTOR_SZ) / 1024);
    return ESP_OK;
}

static TickType_t ticks_until(int64_t deadline_us)
{
    const int64_t left = deadline_us - esp_timer_get_time();
    return left > 0 ? pdMS_TO_TICKS(left / 1000) : 0;
}

static esp_err_t write_direct(uint32_t lba, const void *buf, size_t count, int64_t deadline_us)
{
    esp_err_t err = ESP_ERR_TIMEOUT;
    do
    {
        const int64_t t0 = esp_timer_get_time();
        if (xSemaphoreTake(s_lock, ticks_until(deadline_us)) != pdTRUE)
        {
            return ESP_ERR_TIMEOUT;
        }
        err = card_write_locked(lba, buf, count);
        xSemaphoreGive(s_lock);
        const int64_t t1 = esp_timer_get_time();
        beamer_msc_ring_push(BEAMER_MSC_OP_FLUSH, lba, (uint16_t)count, t0, t1, err);
        if (err == ESP_OK)
        {
            break;
        }
        vTaskDelay(pdMS_TO_TICKS(WBC_STALL_SLICE_MS));
    } while (esp_timer_get_time() < deadline_us);
    return err;
}

esp_err_t beamer_wbc_write(uint32_t lba, const void *buf, size_t count, int64_t deadline_us)
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
        return write_direct(lba, buf, count, deadline_us);
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
            s_meta[slot].seq = ++s_seq;
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
            while (!room && esp_timer_get_time() < deadline_us)
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

esp_err_t beamer_wbc_read(uint32_t lba, void *buf, size_t count, int64_t deadline_us)
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

        esp_err_t err;
        while (true)
        {
            const int64_t t0 = esp_timer_get_time();
            const TickType_t wait = deadline_us ? ticks_until(deadline_us) : pdMS_TO_TICKS(5000);
            if (xSemaphoreTake(s_lock, wait) != pdTRUE)
            {
                return ESP_ERR_TIMEOUT;
            }
            const uint32_t waited = (uint32_t)(esp_timer_get_time() - t0);
            atomic_fetch_add(&s_read_wait_us, waited);
            uint32_t seen = atomic_load(&s_read_wait_max_us);
            while (waited > seen &&
                   !atomic_compare_exchange_weak(&s_read_wait_max_us, &seen, waited))
            {
            }

            err = sdmmc_read_sectors(s_card, dst + i * WBC_SECTOR_SZ, lba + i, run);
            card_result_locked(err, false);
            xSemaphoreGive(s_lock);
            if (err == ESP_OK || esp_timer_get_time() >= deadline_us)
            {
                break;
            }
            vTaskDelay(pdMS_TO_TICKS(WBC_STALL_SLICE_MS));
        }
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

void beamer_wbc_invalidate_all(void)
{
    if (s_meta_lock == NULL)
    {
        return;
    }

    xSemaphoreTake(s_meta_lock, portMAX_DELAY);
    for (int i = 0; i < WBC_SECTORS; i++)
    {
        mark_clean(i);
        s_meta[i].valid = false;
    }
    xSemaphoreGive(s_meta_lock);
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
        beamer_wbc_invalidate_all();
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

uint32_t beamer_wbc_write_failures(void)
{
    return atomic_load(&s_write_failures);
}

uint32_t beamer_wbc_busy_timeouts(void)
{
    return atomic_load(&s_busy_timeouts);
}

bool beamer_wbc_writes_healthy(void)
{
    return atomic_load(&s_writes_healthy);
}

uint32_t beamer_wbc_read_wait_us(void)
{
    return atomic_load(&s_read_wait_us);
}

uint32_t beamer_wbc_read_wait_max_us(void)
{
    return atomic_load(&s_read_wait_max_us);
}

void beamer_wbc_read_wait_reset(void)
{
    atomic_store(&s_read_wait_us, 0);
    atomic_store(&s_read_wait_max_us, 0);
}
