//! Interactive native My Lab menu probe, with wake-aware button input.
//! Usage: cargo run --release --example my_lab_interactive_probe -- <rom.bin> <key-file> <screenshots-dir>
//! Commands (one per line): 100 (million steps), a/b/c (press), s (screenshot), q.
use capybara::capture::{screenshot_png, CaptureFrame};
use capybara::emulator::{my_lab_sd::Card, Machine};
use std::io::{self, BufRead};
use std::path::PathBuf;

fn run(machine: &mut Machine, count: u64) {
    for _ in 0..count { machine.step(); }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert!(args.len() == 4 || args.len() == 5);
    let mut machine = Machine::new();
    let key = std::fs::read_to_string(&args[2]).expect("read device key");
    machine.device_key = Some(u32::from_str_radix(key.trim().trim_start_matches("0x"), 16).unwrap());
    machine.load_firmware_file(&args[1]).expect("load My Lab ROM");
    if args.len() == 5 {
        machine.my_lab_card = Some(Card::open(PathBuf::from(&args[4])).expect("mount My Lab SD card"));
    }
    let out = PathBuf::from(&args[3]);
    std::fs::create_dir_all(&out).unwrap();
    let mut number = 0;
    for line in io::stdin().lock().lines() {
        for command in line.unwrap().split_whitespace() {
            if command == "q" { return; }
            if command == "s" {
                let panel = &machine.periph.display;
                let frame = CaptureFrame::from_rgb565(panel.width as u32, panel.height as u32,
                    &panel.vram, panel.is_enabled).unwrap();
                let file = out.join(format!("{number}.png"));
                screenshot_png(&file, &frame, 4).unwrap();
                eprintln!("saved {}", file.display());
                number += 1;
            } else if let Ok(millions) = command.parse::<u64>() {
                run(&mut machine, millions * 1_000_000);
            } else if let Some(button) = match command {
                "a" => Some(Machine::BOUTON_A), "b" => Some(Machine::BOUTON_B),
                "c" => Some(Machine::BOUTON_C), _ => None,
            } {
                machine.appuyer(button);
                run(&mut machine, 12_000_000);
                machine.relacher(button);
                run(&mut machine, 20_000_000);
            } else { panic!("unknown command: {command}"); }
            eprintln!("pc={:#010x} frames={} asleep={}", machine.cpu.regs.pc,
                machine.periph.display.trames, machine.en_veille_profonde());
        }
    }
}
