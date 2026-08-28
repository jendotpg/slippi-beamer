/*
 * Operates a completely halted station. Runs with FreeRTOS dead, interrupts off
 * and the other core stalled. ROM functions and raw register writes only,
 * all in IRAM.
 */

#include <stdbool.h>
#include <stdint.h>

#include "esp_attr.h"
#include "esp_rom_gpio.h"
#include "esp_rom_sys.h"
#include "soc/gpio_reg.h"
#include "soc/gpio_sig_map.h"

#define PIN_BACKLIGHT 38
#define PIN_LED_CLK 39
#define PIN_LED_DATA 40

#define OUT1_BIT(pin) (1UL << ((pin) - 32))

#define LED_BRIGHTNESS 6

static IRAM_ATTR void pin_out(uint32_t pin)
{
    esp_rom_gpio_pad_select_gpio(pin);
    esp_rom_gpio_connect_out_signal(pin, SIG_GPIO_OUT_IDX, false, false);
    REG_WRITE(GPIO_ENABLE1_W1TS_REG, OUT1_BIT(pin));
}

static IRAM_ATTR void pin_set(uint32_t pin, bool high)
{
    REG_WRITE(high ? GPIO_OUT1_W1TS_REG : GPIO_OUT1_W1TC_REG, OUT1_BIT(pin));
}

static IRAM_ATTR void led_bit(bool high)
{
    pin_set(PIN_LED_DATA, high);
    esp_rom_delay_us(1);
    pin_set(PIN_LED_CLK, true);
    esp_rom_delay_us(1);
    pin_set(PIN_LED_CLK, false);
}

static IRAM_ATTR void led_byte(uint8_t b)
{
    for (int i = 7; i >= 0; i--)
    {
        led_bit((b >> i) & 1);
    }
}

static IRAM_ATTR void tombstone(void)
{
    pin_out(PIN_BACKLIGHT);
    pin_set(PIN_BACKLIGHT, true); /* active low, so high is off */

    pin_out(PIN_LED_CLK);
    pin_out(PIN_LED_DATA);
    pin_set(PIN_LED_CLK, false);

    for (int i = 0; i < 4; i++)
    {
        led_byte(0x00);
    }
    led_byte(0xE0 | (LED_BRIGHTNESS & 0x1F));
    led_byte(0x00); /* blue */
    led_byte(0x00); /* green */
    led_byte(0xFF); /* red */
    for (int i = 0; i < 4; i++)
    {
        led_byte(0xFF);
    }
}

void __real_esp_panic_handler(void *info);

void IRAM_ATTR __wrap_esp_panic_handler(void *info)
{
    static bool s_painted;
    if (!s_painted)
    {
        s_painted = true;
        tombstone();
    }
    __real_esp_panic_handler(info);
}
