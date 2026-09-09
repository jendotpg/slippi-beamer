#include "beamer_msc.h"

#include "esp_attr.h"
#include "esp_heap_caps.h"

static volatile uint32_t s_count;
static volatile uint32_t s_largest;

static IRAM_ATTR void oom_hook(size_t size, uint32_t caps, const char *fn)
{
    (void)caps;
    (void)fn;

    s_count++;
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
