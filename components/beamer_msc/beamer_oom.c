#include "beamer_msc.h"

#include "esp_attr.h"
#include "esp_heap_caps.h"

static volatile uint32_t s_count;
static volatile uint32_t s_largest;
static volatile uint32_t s_last_size;
static volatile uint32_t s_last_caps;
static const char *volatile s_last_fn;

static IRAM_ATTR void oom_hook(size_t size, uint32_t caps, const char *fn)
{
    s_count++;
    s_last_size = (uint32_t)size;
    s_last_caps = caps;
    s_last_fn = fn;
    if (size > s_largest)
    {
        s_largest = (uint32_t)size;
    }
}

void beamer_oom_watch(void)
{
    heap_caps_register_failed_alloc_callback(oom_hook);
}

uint32_t beamer_oom_count(void)
{
    return s_count;
}

uint32_t beamer_oom_largest(void)
{
    return s_largest;
}

uint32_t beamer_oom_last_size(void)
{
    return s_last_size;
}

uint32_t beamer_oom_last_caps(void)
{
    return s_last_caps;
}

const char *beamer_oom_last_fn(void)
{
    return s_last_fn ? s_last_fn : "";
}
