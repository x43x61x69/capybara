#include "h717_board.h"
#include <Wire.h>
#include <driver/spi_master.h>
#include <esp_heap_caps.h>
#include <cstring>

namespace {
spi_device_handle_t display;
uint16_t *bounce;
constexpr size_t bounce_pixels = 536 * 8;

bool command(uint8_t cmd, const uint8_t *data = nullptr, size_t size = 0) {
    spi_transaction_t t{};
    t.cmd = 0x02;
    t.addr = uint32_t(cmd) << 8;
    t.flags = SPI_TRANS_MULTILINE_CMD | SPI_TRANS_MULTILINE_ADDR;
    t.tx_buffer = data;
    t.length = size * 8;
    digitalWrite(board::cs, LOW);
    const bool ok = spi_device_polling_transmit(display, &t) == ESP_OK;
    digitalWrite(board::cs, HIGH);
    return ok;
}

bool value(uint8_t cmd, uint8_t data) { return command(cmd, &data, 1); }

bool window(int x, int y, int w, int h) {
    uint8_t col[] = {uint8_t(x >> 8), uint8_t(x), uint8_t((x+w-1) >> 8), uint8_t(x+w-1)};
    uint8_t row[] = {uint8_t(y >> 8), uint8_t(y), uint8_t((y+h-1) >> 8), uint8_t(y+h-1)};
    return command(0x2A, col, sizeof(col)) && command(0x2B, row, sizeof(row));
}

void pixels(const uint16_t *data, size_t size) {
    spi_transaction_t t{};
    t.flags = SPI_TRANS_MODE_QIO;
    t.cmd = 0x32;
    t.addr = 0x002C00;
    t.tx_buffer = data;
    t.length = size * 16;
    digitalWrite(board::cs, LOW);
    ESP_ERROR_CHECK(spi_device_polling_transmit(display, &t));
    digitalWrite(board::cs, HIGH);
}

uint16_t wire_color(uint16_t v) { return uint16_t((v >> 8) | (v << 8)); }

// Original compact 5x7 glyphs. Only the ASCII labels used by this frontend.
const char *alphabet = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ+-.:/% ";
const uint8_t glyphs[][5] = {
 {0x3e,0x51,0x49,0x45,0x3e},{0x00,0x42,0x7f,0x40,0x00},
 {0x42,0x61,0x51,0x49,0x46},{0x21,0x41,0x45,0x4b,0x31},
 {0x18,0x14,0x12,0x7f,0x10},{0x27,0x45,0x45,0x45,0x39},
 {0x3c,0x4a,0x49,0x49,0x30},{0x01,0x71,0x09,0x05,0x03},
 {0x36,0x49,0x49,0x49,0x36},{0x06,0x49,0x49,0x29,0x1e},
 {0x7e,0x11,0x11,0x11,0x7e},{0x7f,0x49,0x49,0x49,0x36},
 {0x3e,0x41,0x41,0x41,0x22},{0x7f,0x41,0x41,0x22,0x1c},
 {0x7f,0x49,0x49,0x49,0x41},{0x7f,0x09,0x09,0x09,0x01},
 {0x3e,0x41,0x49,0x49,0x7a},{0x7f,0x08,0x08,0x08,0x7f},
 {0x00,0x41,0x7f,0x41,0x00},{0x20,0x40,0x41,0x3f,0x01},
 {0x7f,0x08,0x14,0x22,0x41},{0x7f,0x40,0x40,0x40,0x40},
 {0x7f,0x02,0x0c,0x02,0x7f},{0x7f,0x04,0x08,0x10,0x7f},
 {0x3e,0x41,0x41,0x41,0x3e},{0x7f,0x09,0x09,0x09,0x06},
 {0x3e,0x41,0x51,0x21,0x5e},{0x7f,0x09,0x19,0x29,0x46},
 {0x46,0x49,0x49,0x49,0x31},{0x01,0x01,0x7f,0x01,0x01},
 {0x3f,0x40,0x40,0x40,0x3f},{0x1f,0x20,0x40,0x20,0x1f},
 {0x3f,0x40,0x38,0x40,0x3f},{0x63,0x14,0x08,0x14,0x63},
 {0x07,0x08,0x70,0x08,0x07},{0x61,0x51,0x49,0x45,0x43},
 {0x08,0x08,0x3e,0x08,0x08},{0x08,0x08,0x08,0x08,0x08},
 {0x00,0x60,0x60,0x00,0x00},{0x00,0x36,0x36,0x00,0x00},
 {0x20,0x10,0x08,0x04,0x02},{0x23,0x13,0x08,0x64,0x62},
 {0,0,0,0,0}
};
}

bool board_begin() {
    pinMode(board::power, OUTPUT);
    digitalWrite(board::power, HIGH);
    pinMode(board::cs, OUTPUT);
    digitalWrite(board::cs, HIGH);
    pinMode(board::reset, OUTPUT);
    digitalWrite(board::reset, HIGH);
    delay(200);
    digitalWrite(board::reset, LOW);
    delay(300);
    digitalWrite(board::reset, HIGH);
    delay(200);
    bounce = static_cast<uint16_t *>(heap_caps_malloc(bounce_pixels * 2, MALLOC_CAP_DMA | MALLOC_CAP_INTERNAL));
    if (!bounce) return false;
    spi_bus_config_t bus{};
    bus.data0_io_num = board::d0;
    bus.data1_io_num = board::d1;
    bus.data2_io_num = board::d2;
    bus.data3_io_num = board::d3;
    bus.data4_io_num = bus.data5_io_num = bus.data6_io_num = bus.data7_io_num = -1;
    bus.sclk_io_num = board::clk;
    bus.max_transfer_sz = bounce_pixels * 2;
    bus.flags = SPICOMMON_BUSFLAG_MASTER | SPICOMMON_BUSFLAG_GPIO_PINS;
    if (spi_bus_initialize(SPI3_HOST, &bus, SPI_DMA_CH_AUTO) != ESP_OK) return false;
    spi_device_interface_config_t device{};
    device.command_bits = 8;
    device.address_bits = 24;
    device.mode = 0;
    device.clock_speed_hz = 20000000;
    device.spics_io_num = -1;
    device.flags = SPI_DEVICE_HALFDUPLEX;
    device.queue_size = 1;
    if (spi_bus_add_device(SPI3_HOST, &device, &display) != ESP_OK) return false;
    if (!value(0xFE, 0) || !command(0x11)) return false;
    delay(120);
    // Initialize the RM67162 vendor registers before selecting landscape RGB565.
    if (!value(0xFE, 5) || !value(0x05, 5) || !value(0xFE, 1) || !value(0x73, 0x25)
        || !value(0xFE, 0) || !value(0x36, 0x60) || !value(0x3A, 0x55)
        || !value(0x51, 0) || !command(0x29)) return false;
    delay(120);
    if (!value(0x51, 0xC0)) return false;
    pinMode(board::boot, INPUT_PULLUP);
    pinMode(board::touch_irq, INPUT_PULLUP);
    Wire.begin(board::touch_sda, board::touch_scl, 100000);
    Wire.setTimeOut(4);
    // CST816 disable auto-sleep. A failed probe leaves physical/USB controls usable.
    Wire.beginTransmission(0x15);
    Wire.write(0xFE);
    Wire.write(1);
    const uint8_t touch_status = Wire.endTransmission();
    Serial.printf("CST816 I2C status=%u\n", unsigned(touch_status));
    return true;
}

void board_rect(int x, int y, int w, int h, uint16_t color) {
    if (!display || !bounce || x < 0 || y < 0 || w <= 0 || h <= 0
        || x+w > board::width || y+h > board::height) return;
    const uint16_t value = wire_color(color);
    for (size_t i = 0; i < bounce_pixels; ++i) bounce[i] = value;
    const int rows = bounce_pixels / size_t(w);
    for (int off = 0; off < h; off += rows) {
        const int n = min(rows, h-off);
        if (!window(x, y+off, w, n)) return;
        pixels(bounce, size_t(w) * n);
    }
}

void board_clear(uint16_t color) { board_rect(0, 0, board::width, board::height, color); }

void board_text(int x, int y, const char *text, uint16_t color, int scale) {
    if (!text || scale < 1) return;
    for (; *text; ++text, x += 6*scale) {
        char c = *text;
        if (c >= 'a' && c <= 'z') c -= 32;
        const char *p = strchr(alphabet, c);
        const uint8_t *g = glyphs[p ? size_t(p-alphabet) : 42];
        for (int col = 0; col < 5; ++col)
            for (int row = 0; row < 7; ++row)
                if (g[col] & (1 << row)) board_rect(x+col*scale, y+row*scale, scale, scale, color);
    }
}

bool board_game(const uint16_t *source) {
    if (!display || !bounce || !source) return false;
    constexpr int rows_per_chunk = bounce_pixels / board::game_size;
    for (int y = 0; y < board::game_size; y += rows_per_chunk) {
        const int rows = min(rows_per_chunk, board::game_size-y);
        for (int row = 0; row < rows; ++row) {
            const int sy = ((y+row)*128) / board::game_size;
            for (int x = 0; x < board::game_size; ++x)
                bounce[row*board::game_size+x] = wire_color(source[sy*128+(x*128)/board::game_size]);
        }
        if (!window(board::game_x, board::game_y+y, board::game_size, rows)) return false;
        pixels(bounce, board::game_size*rows);
    }
    return true;
}

bool board_touch(int &x, int &y) {
    Wire.beginTransmission(0x15);
    Wire.write(0x02);
    if (Wire.endTransmission(false) != 0) return false;
    if (Wire.requestFrom(uint8_t(0x15), uint8_t(5)) != 5) return false;
    uint8_t data[5];
    for (auto &v : data) v = Wire.read();
    if (data[0] != 1 || (data[1] & 0xC0) == 0x40) return false;
    // The 1.91-inch panel's MADCTL 0x60 matches the CST816's native axes.
    x = ((data[1] & 0x0F) << 8) | data[2];
    y = ((data[3] & 0x0F) << 8) | data[4];
    return x < board::width && y < board::height;
}
