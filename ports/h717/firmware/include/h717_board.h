#pragma once
#include <Arduino.h>

// H717 = T-Display-S3 AMOLED 1.91-inch TOUCH, not T4-S3 or AMOLED Plus.
namespace board {
constexpr int width = 536, height = 240;
constexpr int cs = 6, clk = 47, d0 = 18, d1 = 7, d2 = 48, d3 = 5;
constexpr int reset = 17, power = 38;
constexpr int touch_sda = 3, touch_scl = 2, touch_irq = 21;
constexpr int boot = 0;
constexpr int uart_tx = 43, uart_rx = 44;
constexpr uint32_t uart_baud = 460800;
constexpr int game_x = 156, game_y = 8, game_size = 224;
}

bool board_begin();
void board_clear(uint16_t color = 0);
void board_rect(int x, int y, int w, int h, uint16_t color);
void board_text(int x, int y, const char *text, uint16_t color = 0xFFFF, int scale = 2);
bool board_game(const uint16_t *pixels);
bool board_touch(int &x, int &y);
