/*
 * A read-only FatFs diskio implementation that is safe while the host owns
 * the medium. We roll our own to
 *   a) separete the read and write locks
 *   b) check reads against our write-back cache (see beamer_wbc.c)
 */

#include "beamer_msc.h"

#include "diskio_impl.h"
#include "esp_log.h"
#include "ffconf.h"

static const char *TAG = "beamer_fat";
static sdmmc_card_t *s_card;

static DSTATUS ro_initialize(BYTE pdrv)
{
    (void)pdrv;
    return 0;
}

static DSTATUS ro_status(BYTE pdrv)
{
    (void)pdrv;
    return 0;
}

static DRESULT ro_read(BYTE pdrv, BYTE *buff, DWORD sector, UINT count)
{
    (void)pdrv;

    const esp_err_t err = beamer_wbc_read((uint32_t)sector, buff, count);
    if (err != ESP_OK)
    {
        ESP_LOGE(TAG, "read %u+%u failed: 0x%x", (unsigned)sector, (unsigned)count, (int)err);
        return RES_ERROR;
    }
    return RES_OK;
}

static DRESULT ro_write(BYTE pdrv, const BYTE *buff, DWORD sector, UINT count)
{
    (void)pdrv;
    (void)buff;
    (void)sector;
    (void)count;
    ESP_LOGE(TAG, "write %u+%u refused: this view is read-only", (unsigned)sector,
             (unsigned)count);
    return RES_WRPRT;
}

static DRESULT ro_ioctl(BYTE pdrv, BYTE cmd, void *buff)
{
    (void)pdrv;

    if (s_card == NULL)
    {
        return RES_NOTRDY;
    }

    switch (cmd)
    {
    case CTRL_SYNC:
        return RES_OK;
    case GET_SECTOR_COUNT:
        *((DWORD *)buff) = (DWORD)s_card->csd.capacity;
        return RES_OK;
    case GET_SECTOR_SIZE:
        *((WORD *)buff) = (WORD)s_card->csd.sector_size;
        return RES_OK;
    case GET_BLOCK_SIZE:
        *((DWORD *)buff) = 1;
        return RES_OK;
    default:
        return RES_PARERR;
    }
}

static const ff_diskio_impl_t s_impl = {
    .init = &ro_initialize,
    .status = &ro_status,
    .read = &ro_read,
    .write = &ro_write,
    .ioctl = &ro_ioctl,
};

esp_err_t beamer_fat_ro_register(uint8_t pdrv, sdmmc_card_t *card)
{
    if (card == NULL)
    {
        return ESP_ERR_INVALID_ARG;
    }
    s_card = card;
    ff_diskio_register(pdrv, &s_impl);
    return ESP_OK;
}
