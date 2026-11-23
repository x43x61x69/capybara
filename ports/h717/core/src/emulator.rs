// Reuse tested instruction/peripheral models; do not fork their decoding fixes.
#[path = "../../../../src/emulator/cpu/mod.rs"]
pub mod cpu;
#[path = "../../../../src/emulator/mmu/mod.rs"]
pub mod mmu;
#[path = "../../../../src/emulator/peripherals/mod.rs"]
pub mod peripherals;
