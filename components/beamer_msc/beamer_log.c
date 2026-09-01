/*
 * esp_log redirected into a journal for later read-back. Lines land in a RAM
 * ring, a priority-1 task in journal.rs drains them, and they reach a human
 * as the `[previous boot log]` section of LOGS/debug_N.txt on reboot
 *
 * HOOK MUST NOT BLOCK.
 */

#include "beamer_msc.h"

#include <stdarg.h>
#include <stdatomic.h>
#include <stdio.h>
#include <string.h>

#include "esp_log.h"
#include "freertos/FreeRTOS.h"

#define BEAMER_LOG_RING 8192

#define BEAMER_LOG_LINE 192

static char s_ring[BEAMER_LOG_RING];
static unsigned s_head; // total bytes written
static unsigned s_tail; // total bytes drained or dropped
static atomic_uint s_dropped;
static portMUX_TYPE s_mux = portMUX_INITIALIZER_UNLOCKED;
static bool s_installed;

static void push(const char *src, size_t n)
{
    if (n == 0 || n > BEAMER_LOG_RING)
    {
        return;
    }

    portENTER_CRITICAL_SAFE(&s_mux);

    const unsigned used = s_head - s_tail; // drops the oldest
    if (used + n > BEAMER_LOG_RING)
    {
        const unsigned need = used + (unsigned)n - BEAMER_LOG_RING;
        s_tail += need;
        atomic_fetch_add_explicit(&s_dropped, need, memory_order_relaxed);
    }

    for (size_t i = 0; i < n; i++)
    {
        s_ring[(s_head + i) % BEAMER_LOG_RING] = src[i];
    }
    s_head += (unsigned)n;

    portEXIT_CRITICAL_SAFE(&s_mux);
}

static int hook(const char *fmt, va_list args)
{
    char line[BEAMER_LOG_LINE];

    const int n = vsnprintf(line, sizeof(line), fmt, args);
    if (n <= 0)
    {
        return n;
    }
    const size_t len = (size_t)n < sizeof(line) - 1 ? (size_t)n : sizeof(line) - 1;
    push(line, len);

    return n;
}

void beamer_log_push(const char *s, size_t n)
{
    push(s, n);
}

void beamer_log_install(void)
{
    if (s_installed)
    {
        return;
    }
    s_installed = true;
    esp_log_set_vprintf(hook);
}

size_t beamer_log_drain(char *out, size_t max)
{
    if (out == NULL || max == 0)
    {
        return 0;
    }

    portENTER_CRITICAL_SAFE(&s_mux);
    unsigned used = s_head - s_tail;
    if (used > max)
    {
        used = (unsigned)max;
    }
    for (unsigned i = 0; i < used; i++)
    {
        out[i] = s_ring[(s_tail + i) % BEAMER_LOG_RING];
    }
    s_tail += used;
    portEXIT_CRITICAL_SAFE(&s_mux);

    return used;
}

uint32_t beamer_log_dropped(void)
{
    return atomic_load(&s_dropped);
}
