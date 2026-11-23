#ifndef H717_CORE_H
#define H717_CORE_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct H717Machine H717Machine;
typedef bool (*H717Read)(void *context, uint32_t offset, uint8_t *out, size_t length);
typedef struct {
    uint64_t cycles, instructions, recompiled_instructions;
    uint32_t pc, frames, baud, sleeping, error;
} H717Stats;
/* All calls on a machine must run on its owning task, never concurrently.
 * Reader/context remain valid until destroy. Memory buffers must be valid for
 * their documented length and must not overlap machine-owned memory. */
H717Machine *h717_create_reader(H717Read read, void *context, size_t image_length);
void h717_destroy(H717Machine *machine);
int h717_step(H717Machine *machine, uint32_t instructions);
void h717_buttons(H717Machine *machine, uint8_t mask); /* A B C wheel bits 0..3 */
void h717_dial(H717Machine *machine, int32_t delta);
bool h717_frame(H717Machine *machine, uint16_t *rgb565_128x128);
size_t h717_uart_rx(H717Machine *machine, const uint8_t *bytes, size_t length);
size_t h717_uart_tx(H717Machine *machine, uint8_t *bytes, size_t capacity);
void h717_stats(H717Machine *machine, H717Stats *stats);
/* Seconds since this host boot; supplies battery-backed RTC during guest sleep.
 * First call establishes a baseline; call periodically with monotonic seconds. */
void h717_clock(H717Machine *machine, uint32_t seconds);
/* Save contains changed flash pages and battery RTC, not a CPU snapshot.
 * Query required bytes with out=NULL,capacity=0. Buffer too small returns 0.
 * Restore BEFORE executing the guest; CRC/ROM mismatch returns false. */
size_t h717_save(H717Machine *machine, uint8_t *out, size_t capacity);
bool h717_restore_save(H717Machine *machine, const uint8_t *data, size_t length);
#ifdef __cplusplus
}
#endif
#endif
