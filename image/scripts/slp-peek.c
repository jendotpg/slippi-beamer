/*
 * slp-peek - answer two questions about a Slippi replay, as cheaply as possible:
 *
 *     which character, colour and nametag is each port playing?
 *     is the game still being played?
 *
 * Prints one line of JSON on stdout, exits 0. Exits 1 on anything it cannot
 * parse, having printed nothing to stdout. Callers must treat exit 1 as "do not
 * publish": a truncated or garbage file is exactly what we keep out of the web
 * root. Reads a stream - never seeks within a file.
 */

#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>

#define BUFSZ 1024
#define GS_DEEPEST 0x1A1
_Static_assert(15 + 1 + 255 + GS_DEEPEST <= BUFSZ, "BUFSZ too small");

static const unsigned char MAGIC[11] = {
    '{', 'U', 0x03, 'r', 'a', 'w', '[', '$', 'U', '#', 'l'
};

/* --- Melee tables --------------------------------------------------------
 *
 * Transcribed verbatim from replay-manager-for-slippi/src/common/constants.ts
 * (characterNames and parryggCharacterColors). The Slippi spec deliberately has
 * no costume-to-colour table because indices are character-specific - blue
 * Young Link and blue Falcon are different numbers - so that file is the
 * authority. Index 0 is the default costume and has no colour name.
 */
static const char *const C_FALCON[]  = {NULL, "black", "red", "white", "green", "blue"};
static const char *const C_DK[]      = {NULL, "black", "red", "blue", "green"};
static const char *const C_FOX[]     = {NULL, "red", "blue", "green"};
static const char *const C_GW[]      = {NULL, "red", "blue", "green"};
static const char *const C_KIRBY[]   = {NULL, "yellow", "blue", "red", "green", "white"};
static const char *const C_BOWSER[]  = {NULL, "red", "blue", "black"};
static const char *const C_LINK[]    = {NULL, "red", "blue", "black", "white"};
static const char *const C_LUIGI[]   = {NULL, "white", "blue", "red"};
static const char *const C_MARIO[]   = {NULL, "yellow", "black", "blue", "green"};
static const char *const C_MARTH[]   = {NULL, "red", "green", "black", "white"};
static const char *const C_MEWTWO[]  = {NULL, "red", "blue", "green"};
static const char *const C_NESS[]    = {NULL, "gold", "blue", "green"};
static const char *const C_PEACH[]   = {NULL, "gold", "white", "blue", "green"};
static const char *const C_PIKACHU[] = {NULL, "red", "blue", "green"};
static const char *const C_ICS[]     = {NULL, "green", "yellow", "red"};
static const char *const C_PUFF[]    = {NULL, "red", "blue", "green", "gold"};
static const char *const C_SAMUS[]   = {NULL, "pink", "dark", "green", "blue"};
static const char *const C_YOSHI[]   = {NULL, "red", "blue", "yellow", "pink", "cyan"};
static const char *const C_ZELDA[]   = {NULL, "red", "blue", "green", "white"};
static const char *const C_SHEIK[]   = {NULL, "red", "blue", "green", "white"};
static const char *const C_FALCO[]   = {NULL, "red", "blue", "green"};
static const char *const C_YL[]      = {NULL, "red", "blue", "white", "black"};
static const char *const C_DOC[]     = {NULL, "red", "blue", "green", "black"};
static const char *const C_ROY[]     = {NULL, "red", "blue", "green", "gold"};
static const char *const C_PICHU[]   = {NULL, "red", "blue", "green"};
static const char *const C_GANON[]   = {NULL, "red", "blue", "green", "purple"};

#define CE(name, tbl) { name, tbl, (unsigned) (sizeof(tbl) / sizeof((tbl)[0])) }

static const struct {
    const char *name;
    const char *const *colors;
    unsigned ncolors;
} CHARS[26] = {
    CE("Falcon",  C_FALCON),  CE("DK",     C_DK),     CE("Fox",     C_FOX),
    CE("GW",      C_GW),      CE("Kirby",  C_KIRBY),  CE("Bowser",  C_BOWSER),
    CE("Link",    C_LINK),    CE("Luigi",  C_LUIGI),  CE("Mario",   C_MARIO),
    CE("Marth",   C_MARTH),   CE("Mewtwo", C_MEWTWO), CE("Ness",    C_NESS),
    CE("Peach",   C_PEACH),   CE("Pikachu", C_PIKACHU), CE("ICs",   C_ICS),
    CE("Puff",    C_PUFF),    CE("Samus",  C_SAMUS),  CE("Yoshi",   C_YOSHI),
    CE("Zelda",   C_ZELDA),   CE("Sheik",  C_SHEIK),  CE("Falco",   C_FALCO),
    CE("YL",      C_YL),      CE("Doc",    C_DOC),    CE("Roy",     C_ROY),
    CE("Pichu",   C_PICHU),   CE("Ganon",  C_GANON),
};

/* --- Shift-JIS -----------------------------------------------------------
 *
 * Nametags are Shift-JIS - they are decoded in full.
 */
static const uint16_t SJIS_81[188] = {
    0x3000, 0x3001, 0x3002, 0xFF0C, 0xFF0E, 0x30FB, 0xFF1A, 0xFF1B,
    0xFF1F, 0xFF01, 0x309B, 0x309C, 0x00B4, 0xFF40, 0x00A8, 0xFF3E,
    0xFFE3, 0xFF3F, 0x30FD, 0x30FE, 0x309D, 0x309E, 0x3003, 0x4EDD,
    0x3005, 0x3006, 0x3007, 0x30FC, 0x2015, 0x2010, 0xFF0F, 0xFF3C,
    0x301C, 0x2016, 0xFF5C, 0x2026, 0x2025, 0x2018, 0x2019, 0x201C,
    0x201D, 0xFF08, 0xFF09, 0x3014, 0x3015, 0xFF3B, 0xFF3D, 0xFF5B,
    0xFF5D, 0x3008, 0x3009, 0x300A, 0x300B, 0x300C, 0x300D, 0x300E,
    0x300F, 0x3010, 0x3011, 0xFF0B, 0x2212, 0x00B1, 0x00D7, 0x00F7,
    0xFF1D, 0x2260, 0xFF1C, 0xFF1E, 0x2266, 0x2267, 0x221E, 0x2234,
    0x2642, 0x2640, 0x00B0, 0x2032, 0x2033, 0x2103, 0xFFE5, 0xFF04,
    0x00A2, 0x00A3, 0xFF05, 0xFF03, 0xFF06, 0xFF0A, 0xFF20, 0x00A7,
    0x2606, 0x2605, 0x25CB, 0x25CF, 0x25CE, 0x25C7, 0x25C6, 0x25A1,
    0x25A0, 0x25B3, 0x25B2, 0x25BD, 0x25BC, 0x203B, 0x3012, 0x2192,
    0x2190, 0x2191, 0x2193, 0x3013, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x2208,
    0x220B, 0x2286, 0x2287, 0x2282, 0x2283, 0x222A, 0x2229, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x2227,
    0x2228, 0x00AC, 0x21D2, 0x21D4, 0x2200, 0x2203, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x2220, 0x22A5, 0x2312, 0x2202, 0x2207, 0x2261, 0x2252,
    0x226A, 0x226B, 0x221A, 0x223D, 0x221D, 0x2235, 0x222B, 0x222C,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x212B,
    0x2030, 0x266F, 0x266D, 0x266A, 0x2020, 0x2021, 0x00B6, 0x0000,
    0x0000, 0x0000, 0x0000, 0x25EF,
};

#define REPLACEMENT 0xFFFDu

static unsigned trail_index(unsigned lo)
{
    return (lo - 0x40u) - (lo > 0x7Fu ? 1u : 0u);
}

static uint32_t sjis_next(const unsigned char *p, unsigned avail, unsigned *used)
{
    unsigned hi = p[0];

    *used = 1;

    if (hi >= 0x20u && hi <= 0x7Eu)
        return hi;                                  
    if (hi >= 0xA1u && hi <= 0xDFu)
        return 0xFF61u + (hi - 0xA1u);   

    if (avail < 2 || hi < 0x81u || hi > 0x83u)
        return REPLACEMENT;

    unsigned lo = p[1];
    if (lo < 0x40u || lo > 0xFCu || lo == 0x7Fu)
        return REPLACEMENT;

    *used = 2;

    if (hi == 0x81u) {
        unsigned i = trail_index(lo);
        if (i < sizeof(SJIS_81) / sizeof(SJIS_81[0]) && SJIS_81[i])
            return SJIS_81[i];
        return REPLACEMENT;
    }

    if (hi == 0x82u) {
        if (lo >= 0x4Fu && lo <= 0x58u) return 0xFF10u + (lo - 0x4Fu);
        if (lo >= 0x60u && lo <= 0x79u) return 0xFF21u + (lo - 0x60u);
        if (lo >= 0x81u && lo <= 0x9Au) return 0xFF41u + (lo - 0x81u);
        if (lo >= 0x9Fu && lo <= 0xF1u) return 0x3041u + (lo - 0x9Fu);
        return REPLACEMENT;
    }

    if (lo <= 0x96u)
        return 0x30A1u + trail_index(lo);
    return REPLACEMENT;
}

static unsigned put_json_utf8(char *out, uint32_t cp)
{
    if (cp == '"' || cp == '\\') {
        out[0] = '\\';
        out[1] = (char) cp;
        return 2;
    }
    if (cp < 0x80u) {
        out[0] = (char) cp;
        return 1;
    }
    if (cp < 0x800u) {
        out[0] = (char) (0xC0u | (cp >> 6));
        out[1] = (char) (0x80u | (cp & 0x3Fu));
        return 2;
    }
    out[0] = (char) (0xE0u | (cp >> 12));
    out[1] = (char) (0x80u | ((cp >> 6) & 0x3Fu));
    out[2] = (char) (0x80u | (cp & 0x3Fu));
    return 3;
}

static unsigned nametag_json(const unsigned char *tag, char *out, unsigned outsz)
{
    unsigned i = 0, n = 0;

    while (i < 16) {
        if (tag[i] == 0)
            break;
        unsigned used;
        uint32_t cp = sjis_next(tag + i, 16 - i, &used);
        if (n + 4 > outsz)
            break;
        n += put_json_utf8(out + n, cp);
        i += used;
    }
    out[n] = '\0';
    return n;
}

/* --- reading -------------------------------------------------------------- */

static uint16_t rd_u16be(const unsigned char *p)
{
    return (uint16_t) (((uint16_t) p[0] << 8) | p[1]);
}

static uint32_t rd_u32be(const unsigned char *p)
{
    return ((uint32_t) p[0] << 24) | ((uint32_t) p[1] << 16) |
           ((uint32_t) p[2] << 8)  |  (uint32_t) p[3];
}

static int fail(const char *msg)
{
    fprintf(stderr, "slp-peek: %s\n", msg);
    return 1;
}

static unsigned slurp(int fd, unsigned char *buf)
{
    unsigned n = 0;
    while (n < BUFSZ) {
        ssize_t r = read(fd, buf + n, BUFSZ - n);
        if (r <= 0)
            break;
        n += (unsigned) r;
    }
    return n;
}

int main(int argc, char **argv)
{
    unsigned char buf[BUFSZ];
    unsigned n;
    int fd = 0;

    if (argc != 2) {
        fprintf(stderr, "usage: %s <file.slp>|-\n", argv[0] ? argv[0] : "slp-peek");
        return 2;
    }

    if (strcmp(argv[1], "-") != 0) {
        fd = open(argv[1], O_RDONLY);
        if (fd < 0)
            return fail("cannot open input");
    }

    n = slurp(fd, buf);
    if (fd != 0)
        close(fd);

    if (n < 17 || memcmp(buf, MAGIC, sizeof(MAGIC)) != 0)
        return fail("not a .slp file");

    int live = (rd_u32be(buf + 11) == 0);

    if (buf[15] != 0x35)
        return fail("no event payloads command");

    unsigned psz = buf[16];
    if (psz < 4 || (psz - 1) % 3 != 0)
        return fail("bad event payloads size");

    unsigned nent = (psz - 1) / 3;
    if (17 + 3 * nent > n)
        return fail("truncated event payloads");

    unsigned gs_size = 0;
    for (unsigned i = 0; i < nent; i++) {
        if (buf[17 + 3 * i] == 0x36) {
            gs_size = rd_u16be(buf + 18 + 3 * i);
            break;
        }
    }

    unsigned gs = 15 + 1 + psz;

    if (gs + 0xD4 >= n || buf[gs] != 0x36)
        return fail("truncated or missing game start");

    int has_nametags = (gs_size + 1 >= GS_DEEPEST);
    if (has_nametags && gs + GS_DEEPEST > n)
        return fail("truncated game start");

    /* --- output ---------------------------------------------------------- */
    printf("{\"live\":%s,\"ports\":[", live ? "true" : "false");

    int first = 1;
    for (unsigned i = 0; i < 4; i++) {
        const unsigned char *pb = buf + gs + 0x65 + 0x24 * i;
        unsigned cid = pb[0], type = pb[1], costume = pb[3];

        if (type != 0 && type != 1)
            continue;

        const char *cname = (cid < 26) ? CHARS[cid].name : NULL;
        const char *color = NULL;
        if (cid < 26 && costume < CHARS[cid].ncolors)
            color = CHARS[cid].colors[costume];

        char tag[52];
        unsigned taglen = 0;
        if (has_nametags)
            taglen = nametag_json(buf + gs + 0x161 + 0x10 * i, tag, sizeof(tag) - 1);

        printf("%s{\"port\":%u,\"char\":", first ? "" : ",", i + 1);
        if (cname) printf("\"%s\"", cname); else printf("null");
        printf(",\"char_id\":");
        if (cid < 26) printf("%u", cid); else printf("null");
        printf(",\"color\":");
        if (color) printf("\"%s\"", color); else printf("null");
        printf(",\"costume\":%u", costume);
        printf(",\"nametag\":");
        if (taglen) printf("\"%s\"", tag); else printf("null");
        printf("}");
        first = 0;
    }

    printf("]}\n");
    return 0;
}
