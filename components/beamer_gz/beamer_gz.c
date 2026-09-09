#include "beamer_gz.h"

#include <stdbool.h>
#include <string.h>

#include "esp_log.h"
#include "zlib.h"

static const char *TAG = "beamer_gz";

#define BEAMER_GZ_WINDOW_BITS 10
#define BEAMER_GZ_MEM_LEVEL 3
#define BEAMER_GZ_ARENA 15360

static __attribute__((aligned(8))) uint8_t s_arena[BEAMER_GZ_ARENA];
static size_t s_used;
static size_t s_high;

static z_stream s_z;
static bool s_open;
static bool s_reported;

static void *gz_alloc(void *opaque, unsigned items, unsigned size)
{
    (void)opaque;

    size_t want = ((size_t)items * size + 7u) & ~(size_t)7u;
    if (want > sizeof(s_arena) - s_used)
    {
        ESP_LOGE(TAG, "arena exhausted: wanted %u B, %u B left of %u",
                 (unsigned)want, (unsigned)(sizeof(s_arena) - s_used),
                 (unsigned)sizeof(s_arena));
        return NULL;
    }

    void *p = &s_arena[s_used];
    s_used += want;
    if (s_used > s_high)
    {
        s_high = s_used;
    }
    return p;
}

static void gz_free(void *opaque, void *addr)
{
    (void)opaque;
    (void)addr;
}

int beamer_gz_begin(int level)
{
    if (s_open)
    {
        beamer_gz_end();
    }

    s_used = 0;
    memset(&s_z, 0, sizeof s_z);
    s_z.zalloc = gz_alloc;
    s_z.zfree = gz_free;

    int r = deflateInit2(&s_z, level, Z_DEFLATED, 16 + BEAMER_GZ_WINDOW_BITS,
                         BEAMER_GZ_MEM_LEVEL, Z_DEFAULT_STRATEGY);
    if (r != Z_OK)
    {
        ESP_LOGE(TAG, "deflateInit2(level %d) failed: %d", level, r);
        return -1;
    }

    s_open = true;
    if (!s_reported)
    {
        s_reported = true;
        ESP_LOGI(TAG, "gzip ready: window %d B, arena %u of %u B",
                 1 << BEAMER_GZ_WINDOW_BITS, (unsigned)s_high,
                 (unsigned)sizeof(s_arena));
    }
    return 0;
}

int beamer_gz_push(const uint8_t *in, size_t in_len, size_t *in_used,
                   uint8_t *out, size_t out_cap, size_t *out_len, int finish)
{
    *in_used = 0;
    *out_len = 0;
    if (!s_open)
    {
        return -1;
    }

    s_z.next_in = (Bytef *)in;
    s_z.avail_in = in_len;
    s_z.next_out = out;
    s_z.avail_out = out_cap;

    int r = deflate(&s_z, finish ? Z_FINISH : Z_NO_FLUSH);

    *in_used = in_len - s_z.avail_in;
    *out_len = out_cap - s_z.avail_out;

    if (r == Z_STREAM_END)
    {
        return 1;
    }

    if (r != Z_OK && r != Z_BUF_ERROR)
    {
        ESP_LOGE(TAG, "deflate failed: %d", r);
        return -1;
    }

    return 0;
}

void beamer_gz_end(void)
{
    if (!s_open)
    {
        return;
    }
    deflateEnd(&s_z);
    s_open = false;
    s_used = 0;
}

size_t beamer_gz_arena_size(void)
{
    return sizeof(s_arena);
}

size_t beamer_gz_arena_high_water(void)
{
    return s_high;
}
