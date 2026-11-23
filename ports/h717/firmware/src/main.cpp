#include <Arduino.h>
#include <LittleFS.h>
#include <Wire.h>
#include <atomic>
#include <driver/uart.h>
#include <esp_heap_caps.h>
#include <esp_partition.h>
#include <esp_spi_flash.h>
#include <freertos/FreeRTOS.h>
#include <freertos/semphr.h>
#include <esp32s3/spiram.h>
#include <h717_core.h>
#include "h717_board.h"

namespace {
constexpr size_t frame_pixels = 128*128;
constexpr size_t max_save = 512*1024+4096;
constexpr uint16_t background = 0x0841, normal = 0x2126, active = 0x047F;
std::atomic<uint8_t> button_mask{0};
std::atomic<int32_t> dial_delta{0};
std::atomic<bool> save_requested{false}, frame_ready{false}, running{false};
std::atomic<int> fatal_error{0}, save_result{0};
std::atomic<uint32_t> tx_count{0}, rx_count{0}, overruns{0};
QueueHandle_t uart_events;
SemaphoreHandle_t frame_mutex, stats_mutex;
uint16_t *shared_frame, *screen_frame;
H717Stats published{};
const esp_partition_t *rom_partition;
const uint8_t *mapped_rom;
spi_flash_mmap_handle_t rom_mapping;
uint32_t rom_length;
std::atomic<bool> filesystem_ready{false};
// The emulation task is the only caller of the Rust allocator.
bool creating_machine = false;

uint32_t little_u32(const uint8_t *p) {
    return uint32_t(p[0]) | uint32_t(p[1])<<8 | uint32_t(p[2])<<16 | uint32_t(p[3])<<24;
}

bool read_rom(void *context, uint32_t offset, uint8_t *out, size_t length) {
    const auto *partition = static_cast<const esp_partition_t *>(context);
    if (!partition || offset > partition->size || length > partition->size-offset) return false;
    if (mapped_rom && partition == rom_partition) {
        if (offset > rom_length || length > rom_length-offset) return false;
        memcpy(out, mapped_rom+offset, length);
        return true;
    }
    return esp_partition_read(partition, offset, out, length) == ESP_OK;
}

bool restore_file(H717Machine *machine, const char *path) {
    File f = LittleFS.open(path, "r");
    if (!f || f.size() == 0 || f.size() > max_save) return false;
    const size_t length = f.size();
    auto *buffer = static_cast<uint8_t *>(heap_caps_malloc(length, MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT));
    if (!buffer) return false;
    const bool read = f.read(buffer, length) == length;
    f.close();
    const bool valid = read && h717_restore_save(machine, buffer, length);
    heap_caps_free(buffer);
    return valid;
}

bool store_save(H717Machine *machine) {
    if (!filesystem_ready) return false;
    const size_t length = h717_save(machine, nullptr, 0);
    if (!length || length > max_save) return false;
    auto *buffer = static_cast<uint8_t *>(heap_caps_malloc(length, MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT));
    if (!buffer) return false;
    if (h717_save(machine, buffer, length) != length) {
        heap_caps_free(buffer);
        return false;
    }
    File f = LittleFS.open("/save.tmp", "w");
    const bool written = f && f.write(buffer, length) == length;
    if (f) { f.flush(); f.close(); }
    heap_caps_free(buffer);
    if (!written) { LittleFS.remove("/save.tmp"); return false; }
    // Keep one known previous copy if power fails between renames.
    if (LittleFS.exists("/save.bin")) {
        if (LittleFS.exists("/save.bak") && !LittleFS.remove("/save.bak")) return false;
        if (!LittleFS.rename("/save.bin", "/save.bak")) return false;
    }
    if (LittleFS.rename("/save.tmp", "/save.bin")) return true;
    if (LittleFS.exists("/save.bak")) LittleFS.rename("/save.bak", "/save.bin");
    return false;
}

void emulation_task(void *) {
    creating_machine = true;
    auto *machine = h717_create_reader(read_rom, const_cast<esp_partition_t *>(rom_partition), rom_length);
    creating_machine = false;
    if (!machine) { fatal_error.store(2); vTaskDelete(nullptr); return; }
    if (filesystem_ready) {
        bool restored = restore_file(machine, "/save.bin");
        if (!restored) restored = restore_file(machine, "/save.bak");
        if (restored) save_result.store(3);
    }
    h717_clock(machine, millis()/1000);
    auto *frame = static_cast<uint16_t *>(heap_caps_malloc(frame_pixels*2, MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT));
    if (!frame) { fatal_error.store(3); h717_destroy(machine); vTaskDelete(nullptr); return; }
    uint8_t rx_pending[512], tx_pending[512];
    size_t rx_used = 0, tx_used = 0, tx_sent = 0;
    uint32_t last_frame = 0, last_stats = 0, last_save = millis(), last_link = 0;
    uint32_t last_yield = millis();
    H717Stats stats{};
    running.store(true);
    for (;;) {
        const uint32_t now = millis();
        uart_event_t event;
        while (xQueueReceive(uart_events, &event, 0) == pdTRUE) {
            if (event.type == UART_FIFO_OVF || event.type == UART_BUFFER_FULL) {
                overruns.fetch_add(1);
                uart_flush_input(UART_NUM_1);
                rx_used = 0;
            }
        }
        h717_buttons(machine, button_mask.load());
        const int32_t delta = dial_delta.exchange(0);
        if (delta) h717_dial(machine, delta);
        h717_clock(machine, now/1000);

        // Keep host RX and TX separate, retaining partial progress on both.
        // The core models Sonix FIFO/interrupt timing; no USB bytes enter this UART.
        if (!rx_used) {
            const int count = uart_read_bytes(UART_NUM_1, rx_pending, sizeof(rx_pending), 0);
            if (count > 0) { rx_used = size_t(count); last_link = now; }
        }
        if (rx_used) {
            const size_t accepted = h717_uart_rx(machine, rx_pending, rx_used);
            if (accepted) {
                rx_count.fetch_add(accepted);
                rx_used -= accepted;
                if (rx_used) memmove(rx_pending, rx_pending+accepted, rx_used);
            }
        }
        if (tx_sent == tx_used) {
            tx_used = h717_uart_tx(machine, tx_pending, sizeof(tx_pending));
            tx_sent = 0;
        }
        if (tx_sent < tx_used) {
            // tx_chars is nonblocking and can report a short FIFO write.
            const int sent = uart_tx_chars(UART_NUM_1, reinterpret_cast<const char *>(tx_pending+tx_sent), tx_used-tx_sent);
            if (sent > 0) { tx_sent += sent; tx_count.fetch_add(sent); last_link = now; }
        }
        const int result = h717_step(machine, 512);
        if (result != 0) {
            fatal_error.store(10+result);
            running.store(false);
            h717_stats(machine, &stats);
            if (xSemaphoreTake(stats_mutex, portMAX_DELAY) == pdTRUE) {
                published = stats;
                xSemaphoreGive(stats_mutex);
            }
            break;
        }
        if (uint32_t(now-last_frame) >= 33) {
            last_frame = now;
            if (h717_frame(machine, frame)) {
                xSemaphoreTake(frame_mutex, portMAX_DELAY);
                memcpy(shared_frame, frame, frame_pixels*2);
                frame_ready.store(true);
                xSemaphoreGive(frame_mutex);
            }
        }
        if (uint32_t(now-last_stats) >= 250) {
            last_stats = now;
            h717_stats(machine, &stats);
            xSemaphoreTake(stats_mutex, portMAX_DELAY);
            published = stats;
            xSemaphoreGive(stats_mutex);
        }
        if (uint32_t(now-last_save) >= 120000) save_requested.store(true);
        // Internal flash writes stall the cache. Never auto-save during a link.
        if (save_requested.load() && stats.baud == 0 && uint32_t(now-last_link) >= 2000 && tx_sent == tx_used && !rx_used) {
            save_requested.store(false);
            save_result.store(store_save(machine) ? 1 : 2);
            last_save = now;
        }
        // Allow the idle task/watchdog to run without sleeping every 512 steps.
        if (uint32_t(now-last_yield) >= 5) { last_yield = now; vTaskDelay(1); }
    }
    heap_caps_free(frame);
    h717_destroy(machine);
    vTaskDelete(nullptr);
}

void button(int x, int y, const char *label, bool down = false) {
    board_rect(x, y, 132, 44, down ? active : normal);
    const int width = strlen(label)*12;
    board_text(x+(132-width)/2, y+15, label);
}

void draw_controls(uint8_t mask) {
    button(8, 8, "A", mask & 1);
    button(8, 60, "B", mask & 2);
    button(8, 112, "C", mask & 4);
    button(8, 164, "A+C", (mask & 5) == 5);
    button(396, 8, "DIAL +");
    button(396, 60, "DIAL -");
    button(396, 112, "WHEEL", mask & 8);
    button(396, 164, "SAVE");
}

void error_screen(int code) {
    board_clear(background);
    board_text(12, 20, "H717 EMULATOR STOPPED", 0xF800);
    char line[44];
    snprintf(line, sizeof(line), "ERROR %d", code);
    board_text(12, 54, line);
    H717Stats stats{};
    if (stats_mutex && xSemaphoreTake(stats_mutex, 0) == pdTRUE) {
        stats = published;
        xSemaphoreGive(stats_mutex);
    }
    snprintf(line, sizeof(line), "PC %08lX CORE %lu", (unsigned long)stats.pc, (unsigned long)stats.error);
    board_text(12, 86, line);
    board_text(12, 130, "SEE USB LOG AND README", 0xFFE0);
    Serial.printf("H717 stopped: frontend=%d core=%lu PC=%08lx\n", code, (unsigned long)stats.error, (unsigned long)stats.pc);
}
}

extern "C" void *h717_alloc(size_t size, size_t alignment) {
    alignment = max(alignment, sizeof(void *));
    if (size > SIZE_MAX-alignment) return nullptr;
    size = max(size, size_t(1));
    // Rust allocation alignment is a power of two. Round size for aligned_alloc.
    size = (size+alignment-1) & ~(alignment-1);
    constexpr uint32_t internal = MALLOC_CAP_INTERNAL | MALLOC_CAP_8BIT;
    constexpr uint32_t external = MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT;
    // Keep CPU state and the program RAM in faster memory when there is room.
    // Leave the ROM cache and later allocations, including saves, in PSRAM.
    const bool prefer_internal = creating_machine && (size <= 8*1024 || size == 64*1024)
        && heap_caps_get_free_size(internal) >= size + 64*1024;
    void *p = heap_caps_aligned_alloc(alignment, size, prefer_internal ? internal : external);
    if (!p) p = heap_caps_aligned_alloc(alignment, size, prefer_internal ? external : internal);
    return p;
}
extern "C" void h717_free(void *ptr, size_t, size_t) { heap_caps_free(ptr); }
extern "C" void h717_panic() {
    fatal_error.store(4);
    running.store(false);
    for (;;) vTaskDelay(pdMS_TO_TICKS(1000));
}

void setup() {
    Serial.begin(115200);
    Serial.setTxTimeoutMs(0);
    if (!psramFound() || esp_spiram_get_size() < 8*1024*1024) {
        Serial.println("H717 requires ESP32-S3R8 with 8 MB OPI PSRAM. Not K191.");
        fatal_error.store(1);
        return;
    }
    if (!board_begin()) { fatal_error.store(5); return; }
    board_clear(background);
    board_text(12, 20, "H717 PARADISE EMULATOR");
    board_text(12, 54, "LOADING ROM", 0x07FF);
    rom_partition = esp_partition_find_first(ESP_PARTITION_TYPE_DATA, ESP_PARTITION_SUBTYPE_ANY, "tamarom");
    uint8_t header[64];
    if (!rom_partition || !read_rom(const_cast<esp_partition_t *>(rom_partition), 0, header, sizeof(header))) {
        fatal_error.store(6); return;
    }
    rom_length = little_u32(header+12);
    if (rom_length < sizeof(header) || rom_length > rom_partition->size) { fatal_error.store(6); return; }
    // Mapping consumes MMU address space, not an equally large PSRAM buffer.
    // Availability depends on SDK/cache configuration; per-page reads remain
    // a functional fallback if the whole package cannot be mapped.
    const void *mapping = nullptr;
    const esp_err_t mapped = esp_partition_mmap(rom_partition, 0, rom_length,
        SPI_FLASH_MMAP_DATA, &mapping, &rom_mapping);
    if (mapped == ESP_OK) mapped_rom = static_cast<const uint8_t *>(mapping);
    Serial.printf("ROM %lu bytes, mmap=%s (fallback=partition reads)\n", (unsigned long)rom_length,
        mapped == ESP_OK ? "yes" : "no");
    frame_mutex = xSemaphoreCreateMutex();
    stats_mutex = xSemaphoreCreateMutex();
    shared_frame = static_cast<uint16_t *>(heap_caps_calloc(frame_pixels, 2, MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT));
    screen_frame = static_cast<uint16_t *>(heap_caps_calloc(frame_pixels, 2, MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT));
    if (!frame_mutex || !stats_mutex || !shared_frame || !screen_frame) { fatal_error.store(3); return; }
    // Formatting is explicit: an existing filesystem is never erased on mount failure.
    filesystem_ready = LittleFS.begin(false);
    if (!filesystem_ready) Serial.println("LittleFS not mounted: format once with USB command F; no auto-format performed.");
    uart_config_t uart{};
    uart.baud_rate = board::uart_baud;
    uart.data_bits = UART_DATA_8_BITS;
    uart.parity = UART_PARITY_DISABLE;
    uart.stop_bits = UART_STOP_BITS_1;
    uart.flow_ctrl = UART_HW_FLOWCTRL_DISABLE;
    uart.source_clk = UART_SCLK_APB;
    if (uart_param_config(UART_NUM_1, &uart) != ESP_OK
        || uart_set_pin(UART_NUM_1, board::uart_tx, board::uart_rx, UART_PIN_NO_CHANGE, UART_PIN_NO_CHANGE) != ESP_OK
        || uart_driver_install(UART_NUM_1, 16384, 0, 20, &uart_events, 0) != ESP_OK) {
        fatal_error.store(7); return;
    }
    uart_set_rx_timeout(UART_NUM_1, 2);
    board_clear(background);
    draw_controls(0);
    Serial.println("BOOT=B; touch A/B/C/A+C, DIAL +/-, WHEEL, SAVE. USB: 1/2/3 hold A/B/C,0 release,+/-,s save.");
    if (xTaskCreatePinnedToCore(emulation_task, "tama", 24576, nullptr, 2, nullptr, 1) != pdPASS) fatal_error.store(3);
}

void loop() {
    static int shown_error = 0;
    const int error = fatal_error.load();
    if (error) {
        if (shown_error != error) { shown_error = error; error_screen(error); }
        delay(50);
        return;
    }
    static uint8_t usb_mask = 0, drawn_mask = 255;
    static uint32_t last_dial = 0, last_status = 0;
    static int last_region = -1;
    static bool was_touching = false;
    static uint64_t last_cycles = 0;
    static uint64_t last_instructions = 0;
    static uint64_t last_recompiled = 0;
    static bool reported_bright_frame = false;
    static bool trace_panel = false;
    while (Serial.available()) {
        const int c = Serial.read();
        if (c >= '1' && c <= '4') usb_mask ^= 1u << (c-'1');
        else if (c == '0') usb_mask = 0;
        else if (c == '+') dial_delta.fetch_add(1);
        else if (c == '-') dial_delta.fetch_sub(1);
        else if (c == 's') save_requested.store(true);
        else if (c == 'p') trace_panel = !trace_panel;
        else if (c == 'F' && !filesystem_ready) {
            // Only a fresh installation with no mounted FS can use this command.
            // Do it before the worker ever accesses a filesystem.
            if (LittleFS.format() && LittleFS.begin(false)) {
                filesystem_ready.store(true, std::memory_order_release);
                Serial.println("Save filesystem initialized.");
            }
        }
        if (trace_panel && c >= '0' && c <= '4') {
            Serial.printf("Panel input ms=%lu mask=%u\n", millis(), unsigned(usb_mask));
        }
    }
    uint8_t mask = usb_mask;
    if (digitalRead(board::boot) == LOW) mask |= 2;
    int x = 0, y = 0, region = -1;
    const bool touching = board_touch(x, y);
    if (touching) {
        const int row = y >= 8 ? (y-8)/52 : -1;
        const bool inside_row = row >= 0 && row < 4 && y < 8+row*52+44;
        if (inside_row && x >= 8 && x < 140) {
            region = row;
            mask |= row == 3 ? 5 : (1 << row);
        } else if (inside_row && x >= 396 && x < 528) {
            region = 4+row;
            if (row == 2) mask |= 8;
            if (row == 3 && region != last_region) save_requested.store(true);
            const uint32_t now = millis();
            if (row < 2 && (region != last_region || uint32_t(now-last_dial) >= 180)) {
                last_dial = now;
                dial_delta.fetch_add(row == 0 ? 1 : -1);
            }
        }
        if (!was_touching || region != last_region) {
            Serial.printf("Touch x=%d y=%d control=%d\n", x, y, region);
        }
    }
    was_touching = touching;
    last_region = region;
    button_mask.store(mask);
    if (drawn_mask != mask) { drawn_mask = mask; draw_controls(mask); }
    if (frame_ready.load() && xSemaphoreTake(frame_mutex, 0) == pdTRUE) {
        memcpy(screen_frame, shared_frame, frame_pixels*2);
        frame_ready.store(false);
        xSemaphoreGive(frame_mutex);
        const bool sent = board_game(screen_frame);
        if (sent && trace_panel) {
            uint32_t fingerprint = 2166136261u;
            for (size_t i = 0; i < frame_pixels; ++i) {
                fingerprint = (fingerprint ^ screen_frame[i]) * 16777619u;
            }
            Serial.printf("Panel frame ms=%lu hash=%08lx\n", millis(), (unsigned long)fingerprint);
        }
        if (sent && !reported_bright_frame) {
            size_t bright_pixels = 0;
            for (size_t i = 0; i < frame_pixels; ++i) {
                const uint16_t rgb = screen_frame[i];
                if ((rgb & 0xF800) >= 0x4000 || (rgb & 0x07E0) >= 0x0200 || (rgb & 0x001F) >= 8) {
                    ++bright_pixels;
                }
            }
            if (bright_pixels >= frame_pixels / 16) {
                Serial.printf("Bright game frame sent to AMOLED (%u/%u pixels).\n",
                    unsigned(bright_pixels), unsigned(frame_pixels));
                reported_bright_frame = true;
            }
        }
    }
    const uint32_t now = millis();
    if (uint32_t(now-last_status) >= 1000 && xSemaphoreTake(stats_mutex, 0) == pdTRUE) {
        const H717Stats stats = published;
        xSemaphoreGive(stats_mutex);
        const uint32_t elapsed = now-last_status;
        // Guest wake/reset may restart its counter; do not display an unsigned
        // wrap as an impossibly high emulation speed.
        const uint64_t cycle_delta = stats.cycles >= last_cycles ? stats.cycles-last_cycles : 0;
        const unsigned speed = unsigned(cycle_delta*100000ULL/(96000000ULL*elapsed));
        const unsigned instructions_per_second = unsigned((stats.instructions-last_instructions)*1000ULL/elapsed);
        const unsigned recompiled_per_second = unsigned((stats.recompiled_instructions-last_recompiled)*1000ULL/elapsed);
        last_cycles = stats.cycles;
        last_instructions = stats.instructions;
        last_recompiled = stats.recompiled_instructions;
        last_status = now;
        board_rect(0, 216, 150, 24, background);
        char text[30];
        snprintf(text, sizeof(text), "%u%% TX%lu", speed, (unsigned long)tx_count.load());
        board_text(8, 222, text, speed >= 95 ? 0x07E0 : 0xFFE0, 1);
        board_rect(388, 216, 148, 24, background);
        const int save = save_result.exchange(0);
        if (save) snprintf(text, sizeof(text), "%s", save == 1 ? "SAVED" : save == 3 ? "RESTORED" : "SAVE ERROR");
        else snprintf(text, sizeof(text), "RX%lu %s", (unsigned long)rx_count.load(), stats.sleeping ? "SLEEP" : "");
        board_text(396, 222, text, save == 2 ? 0xF800 : 0xFFFF, 1);
        Serial.printf("speed=%u%% instructions/s=%u recomp/s=%u pc=%08lx frames=%lu baud=%lu TX=%lu RX=%lu overrun=%lu save=%d freePSRAM=%u\n",
            speed, instructions_per_second, recompiled_per_second, (unsigned long)stats.pc, (unsigned long)stats.frames, (unsigned long)stats.baud,
            (unsigned long)tx_count.load(), (unsigned long)rx_count.load(), (unsigned long)overruns.load(), save, ESP.getFreePsram());
    }
    delay(5);
}
