/*
 * Reads back the last panic. Handles two types:
 *      a) A hard fault -- a stack overflow, an alloc failure, a C abort, a
 *         watchdog -- reaches the ESP-IDF panic handler, dumps core and halts,
 *         and `beamer_panic_take` reads that back after the replug.
 *      b) A Rust panic never gets there: `panic.rs` parks the faulting task
 *         instead, so nothing is written to flash and `beamer_backtrace` has
 *         to walk the stack live, while it still exists.
 */

#include "beamer_msc.h"

#include <stdarg.h>
#include <stdio.h>
#include <string.h>

#include "esp_attr.h"
#include "esp_core_dump.h"
#include "esp_cpu_utils.h"
#include "esp_debug_helpers.h"
#include "esp_memory_utils.h"
#include "esp_log.h"

static const char *TAG = "beamer_panic";

#define BEAMER_BOOT_MAGIC 0xB3A3E12Cu

static RTC_NOINIT_ATTR uint32_t s_boot_magic;
static RTC_NOINIT_ATTR uint32_t s_boot_count;

uint32_t beamer_boot_count(void)
{
    static bool counted;
    if (!counted)
    {
        counted = true;
        if (s_boot_magic != BEAMER_BOOT_MAGIC)
        {
            s_boot_magic = BEAMER_BOOT_MAGIC; // on a cold start this will be garbage data, not our magic
            s_boot_count = 0;
        }
        s_boot_count++;
    }
    return s_boot_count;
}

static void append(char *out, size_t len, size_t *used, const char *fmt, ...)
{
    if (*used >= len)
    {
        return;
    }
    va_list ap;
    va_start(ap, fmt);
    const int n = vsnprintf(out + *used, len - *used, fmt, ap);
    va_end(ap);
    if (n > 0)
    {
        *used += ((size_t)n < len - *used) ? (size_t)n : (len - *used);
    }
}

#define BEAMER_BT_DEPTH 16

static bool frame_sane(const esp_backtrace_frame_t *f)
{
    return esp_stack_ptr_is_sane(f->sp) &&
           esp_ptr_executable((void *)esp_cpu_process_stack_pc(f->pc));
}

size_t beamer_backtrace(char *out, size_t len)
{
    if (out == NULL || len == 0)
    {
        return 0;
    }
    out[0] = '\0';

    esp_backtrace_frame_t frame = {0};
    esp_backtrace_get_start(&frame.pc, &frame.sp, &frame.next_pc);

    size_t used = 0;
    bool corrupted = !frame_sane(&frame);

    append(out, len, &used, "0x%08lx", (unsigned long)esp_cpu_process_stack_pc(frame.pc));

    for (int i = 1; i < BEAMER_BT_DEPTH && !corrupted && frame.next_pc != 0; i++)
    {
        if (!esp_backtrace_get_next_frame(&frame))
        {
            corrupted = true;
        }
        append(out, len, &used, " 0x%08lx", (unsigned long)esp_cpu_process_stack_pc(frame.pc));
    }

    if (corrupted)
    {
        append(out, len, &used, " (corrupt)");
    }
    else if (frame.next_pc != 0)
    {
        append(out, len, &used, " ...");
    }
    return used;
}

#if CONFIG_ESP_COREDUMP_ENABLE_TO_FLASH && CONFIG_ESP_COREDUMP_DATA_FORMAT_ELF

bool beamer_panic_take(char *out, size_t len)
{
    if (out == NULL || len == 0)
    {
        return false;
    }
    out[0] = '\0';

    if (esp_core_dump_image_check() != ESP_OK)
    {
        return false;
    }

    static esp_core_dump_summary_t summary;
    if (esp_core_dump_get_summary(&summary) != ESP_OK)
    {
        ESP_LOGW(TAG, "a core dump is present but its summary would not parse");
        esp_core_dump_image_erase();
        return false;
    }

    size_t used = 0;
    append(out, len, &used, "task %.16s, PC 0x%08lx, cause %lu, vaddr 0x%08lx",
           summary.exc_task, (unsigned long)summary.exc_pc,
           (unsigned long)summary.ex_info.exc_cause,
           (unsigned long)summary.ex_info.exc_vaddr);

    const uint32_t depth = summary.exc_bt_info.depth;
    if (depth > 0)
    {
        append(out, len, &used, "\nbacktrace");
        for (uint32_t i = 0; i < depth && i < 16; i++)
        {
            append(out, len, &used, " 0x%08lx", (unsigned long)summary.exc_bt_info.bt[i]);
        }
        if (summary.exc_bt_info.corrupted)
        {
            append(out, len, &used, " (corrupt)");
        }
    }

    ESP_LOGE(TAG, "previous boot panicked: %s", out);
    ESP_LOGE(TAG, "decode with: xtensa-esp32s3-elf-addr2line -pfiaC -e "
                  "target/xtensa-esp32s3-espidf/release/beamer <PC and backtrace>");

    esp_core_dump_image_erase();
    return true;
}

#else // no coredump!

bool beamer_panic_take(char *out, size_t len)
{
    if (out != NULL && len > 0)
    {
        out[0] = '\0';
    }
    return false;
}

#endif
