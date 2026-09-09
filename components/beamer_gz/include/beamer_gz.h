#pragma once

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C"
{
#endif

   int beamer_gz_begin(int level);

   int beamer_gz_push(const uint8_t *in, size_t in_len, size_t *in_used,
                      uint8_t *out, size_t out_cap, size_t *out_len,
                      int finish);

   void beamer_gz_end(void);

   size_t beamer_gz_arena_size(void);
   size_t beamer_gz_arena_high_water(void);

#ifdef __cplusplus
}
#endif
