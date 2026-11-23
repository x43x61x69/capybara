//! Execute the installed Connection -> UART dispatcher and capture its tool screen.
//! Usage: cargo run --release --example my_lab_uart_launch_probe -- <installed.bin> <output.png>
use capybara::capture::{screenshot_png, CaptureFrame};
use capybara::emulator::{Machine, StepResult};
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert!(args.len() == 3, "usage: my_lab_uart_launch_probe <installed.bin> <output.png>");
    let mut machine = Machine::new();
    let report = machine.load_firmware_file(&args[1]).expect("load updated My Lab ROM");
    assert!(report.bootable);
    let rom = std::fs::read(&args[1]).expect("read updated ROM");
    assert_eq!(&rom[0x1af008..0x1af010], b"PXMLRO01");
    assert_eq!(u32::from_le_bytes(rom[0x1ae124..0x1ae128].try_into().unwrap()), 0x7ffd);
    machine.periph.xip.base = 0x6011_1000;
    machine.periph.xip.ctrl = 3;
    machine.cpu.regs.pc = 0x1009_d000; // stock Connection's new dispatcher
    machine.cpu.regs.msp = 0x1801_f000;
    machine.cpu.regs.set_reg(0, 0x7ffd); // selected UART row
    for n in 0..600_000_000u64 {
        let pc = machine.cpu.regs.pc;
        match machine.step() {
            StepResult::Ok(_) => {},
            _ => { eprintln!("stopped at {pc:#010x}"); break; },
        }
        if n % 50_000_000 == 0 {
            eprintln!("steps={n} pc={:#010x} frames={}", machine.cpu.regs.pc, machine.periph.display.trames);
        }
        if machine.periph.display.trames > 1 { break; }
    }
    let panel = &machine.periph.display;
    let frame = CaptureFrame::from_rgb565(panel.width as u32, panel.height as u32,
        &panel.vram, panel.is_enabled).expect("capture LCD");
    screenshot_png(Path::new(&args[2]), &frame, 4).expect("save UART screen");
    eprintln!("final pc={:#010x} frames={}", machine.cpu.regs.pc, panel.trames);
    assert!(panel.trames > 0, "UART tool did not draw a screen");
}
