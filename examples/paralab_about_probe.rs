//! Capture the My Lab ParaLab About screen from the emulator.
//! Usage: cargo run --example paralab_about_probe -- <image.bin> <output-prefix>
use capybara::capture::{screenshot_png, CaptureFrame};
use capybara::emulator::{Machine, StepResult};
use std::path::Path;

fn shot(machine: &Machine, path: &str) {
    let panel = &machine.periph.display;
    let frame = CaptureFrame::from_rgb565(
        panel.width as u32, panel.height as u32, &panel.vram, panel.is_enabled,
    ).expect("capture LCD");
    screenshot_png(Path::new(path), &frame, 4).expect("save PNG");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert!(args.len() == 3, "usage: paralab_about_probe <image.bin> <output-prefix>");
    let mut machine = Machine::new();
    machine.load_firmware_file(&args[1]).expect("load My Lab image");
    machine.periph.xip.base = 0x6001_1000;
    machine.periph.xip.ctrl = 3;
    machine.cpu.regs.pc = 0x1002_5020; // installed launcher at flash 0x36000
    machine.cpu.regs.msp = 0x1801_f000;
    let prefix = &args[2];
    let mut pressed = false;
    for n in 0..36_000_000u64 {
        if machine.cpu.regs.pc == 0xa21c && machine.cpu.regs.get_reg(0) > 1 {
            machine.cpu.regs.set_reg(0, 1);
        }
        // The menu has seven entries. Move six times, then open About.
        if (4_000_000..28_000_000).contains(&n) {
            let phase = (n - 4_000_000) % 4_000_000;
            if phase == 0 && !pressed { machine.appuyer(Machine::BOUTON_A); pressed = true; }
            if phase == 700_000 && pressed { machine.relacher(Machine::BOUTON_A); pressed = false; }
        }
        if n == 29_000_000 { machine.appuyer(Machine::BOUTON_B); }
        if n == 29_700_000 { machine.relacher(Machine::BOUTON_B); }
        if n % 4_000_000 == 3_999_999 {
            shot(&machine, &format!("{prefix}-{:02}.png", n / 4_000_000));
            eprintln!("step={n} pc={:#010x} frames={}", machine.cpu.regs.pc, machine.periph.display.trames);
        }
        match machine.step() {
            StepResult::Ok(_) => {},
            _ => { eprintln!("stopped at {:#010x}", machine.cpu.regs.pc); break; },
        }
    }
    shot(&machine, &format!("{prefix}-final.png"));
}
