//! Run the stock My Lab SD updater against a host card directory, in memory.
//! Usage: cargo run --example my_lab_update_probe -- <stock.bin> <card-dir> [steps] [expected.bin]
use capybara::emulator::{my_lab_sd::Card, Machine, StepResult};
use capybara::capture::{screenshot_png, CaptureFormat, CaptureFrame, Recorder};
use std::path::PathBuf;

fn lcd_frame(machine: &Machine) -> CaptureFrame {
    let panel = &machine.periph.display;
    CaptureFrame::from_rgb565(panel.width as u32, panel.height as u32,
        &panel.vram, panel.is_enabled).expect("capture My Lab LCD")
}

fn main() {
    let mut args = std::env::args().skip(1);
    let rom = args.next().expect("stock ROM path");
    let card = PathBuf::from(args.next().expect("SD card directory"));
    let budget: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(30_000_000);
    let expected_rom = args.next().map(PathBuf::from);
    let mut machine = Machine::new();
    machine.load_firmware_file(&rom).expect("load stock ROM");
    machine.remplacer_la_pile();
    machine.my_lab_card = Some(Card::open(card).expect("mount SD card"));
    // Hold A+B+C at reset: My Lab uses active-low P1.15/14/13 before the
    // frontend has selected the board-specific button mapping.
    for pin in [15, 14, 13] {
        machine.periph.port1.appuyer(pin);
    }
    let mut previous_console = 0;
    let mut direct_console = String::new();
    let mut previous_reads = 0;
    let mut released = false;
    let mut input_samples = 0;
    let mut previous_scene = u32::MAX;
    let mut update_prompt_entered = None;
    let mut pressed_prompt_a = false;
    let mut prompt_a_done = false;
    let mut pressed_prompt_b = false;
    let movie = std::env::var_os("MP4_PATH").map(PathBuf::from);
    let mut recorder: Option<Recorder> = None;
    for n in 0..budget {
        if n == 70_000_000 {
            if let Some(path) = movie.as_ref() {
                let frame = lcd_frame(&machine);
                screenshot_png(&path.with_extension("language.png"), &frame, 4).expect("save language frame");
                recorder = Some(Recorder::start(path.clone(), CaptureFormat::Mp4, frame).expect("start MP4 recording"));
            }
        }
        if n % 500_000 == 0 {
            if let Some(recorder) = recorder.as_ref() {
                recorder.update_frame(&lcd_frame(&machine)).expect("record LCD frame");
            }
        }
        if n == 1_500_000_000 {
            machine.instantane().ecrire(std::path::Path::new("/tmp/mylab-update-1p5b.json"))
                .expect("save update checkpoint");
            eprintln!("saved checkpoint at steps={n}");
        }
        if n == 80_000_000 { machine.appuyer(Machine::BOUTON_A); }
        if n == 95_000_000 { machine.relacher(Machine::BOUTON_A); }
        if n == 105_000_000 {
            if let Some(path) = movie.as_ref() {
                screenshot_png(&path.with_extension("english.png"), &lcd_frame(&machine), 4)
                    .expect("save English language choice");
            }
        }
        if n == 115_000_000 { machine.appuyer(Machine::BOUTON_B); }
        if n == 140_000_000 { machine.relacher(Machine::BOUTON_B); }
        if matches!(n, 80_000_000 | 90_000_000 | 105_000_000) {
            let keys = u32::from_le_bytes(machine.bus.sram.data[0x1180..0x1184].try_into().unwrap());
            eprintln!("button state steps={n} p1={:#010x} direction={:#010x} keys={keys:#010x}",
                machine.periph.port1.entrees, machine.periph.port1.direction);
            eprintln!("button map={:02x?} sleeping={}",
                &machine.bus.sram.data[0x14..0x17], machine.en_veille_profonde());
        }
        let pc = machine.cpu.regs.pc;
        if matches!(pc, 0x1000_57ce | 0x1000_58f6 | 0x1000_591c | 0x1000_5936 | 0x1000_5980 | 0x1000_3d46) {
            let status = u32::from_le_bytes(machine.bus.sram.data[0x1bfd4..0x1bfd8].try_into().unwrap());
            let detail = u32::from_le_bytes(machine.bus.sram.data[0x1bfd8..0x1bfdc].try_into().unwrap());
            eprintln!("verifier steps={n} pc={pc:#010x} r0={:#010x} status={status:#010x} detail={detail:#010x}",
                machine.cpu.regs.get_reg(0));
        }
        if pc == 0x1000_3d6c {
            let sram = &machine.bus.sram.data;
            let ptr = u32::from_le_bytes(sram[0x11348..0x1134c].try_into().unwrap());
            let index = ptr.wrapping_sub(0x1800_0000) as usize;
            if let Some(bytes) = sram.get(index..index.saturating_add(4)) {
                let scene = u32::from_le_bytes(bytes.try_into().unwrap());
                if scene != previous_scene {
                    eprintln!("scene steps={n} id={scene:#x} context={ptr:#010x} flags={:02x?}",
                        sram.get(index + 0x28..index + 0x2e).unwrap_or(&[]));
                    if let Some(recorder) = recorder.as_ref() {
                        recorder.update_frame(&lcd_frame(&machine)).expect("record scene");
                        std::thread::sleep(std::time::Duration::from_millis(500));
                    }
                    previous_scene = scene;
                }
                if scene == 0x3eb {
                    let entered = *update_prompt_entered.get_or_insert(n);
                    let elapsed = n.saturating_sub(entered);
                    let ui_ptr = u32::from_le_bytes(machine.bus.sram.data[index + 0x1c..index + 0x20].try_into().unwrap());
                    let ui_index = ui_ptr.wrapping_sub(0x1800_0000) as usize;
                    if let Some(bytes) = machine.bus.sram.data.get(ui_index + 0x11c..ui_index + 0x120) {
                        let list_ptr = u32::from_le_bytes(bytes.try_into().unwrap());
                        let list_index = list_ptr.wrapping_sub(0x1800_0000) as usize;
                        if let Some(choice) = machine.bus.sram.data.get(list_index + 4..list_index + 6) {
                            eprintln!("update prompt steps={n} choice={}", u16::from_le_bytes([choice[0], choice[1]]));
                        }
                    }
                    if elapsed >= 3_000_000 && !pressed_prompt_a && !prompt_a_done {
                        machine.appuyer(Machine::BOUTON_A);
                        pressed_prompt_a = true;
                        eprintln!("pressed A to switch update choice at steps={n}");
                    }
                    if elapsed >= 6_000_000 && pressed_prompt_a {
                        machine.relacher(Machine::BOUTON_A);
                        pressed_prompt_a = false;
                        prompt_a_done = true;
                    }
                    if elapsed >= 9_000_000 && !pressed_prompt_b {
                        machine.appuyer(Machine::BOUTON_B);
                        pressed_prompt_b = true;
                        eprintln!("pressed B on switched update choice at steps={n}");
                    }
                } else {
                    update_prompt_entered = None;
                    prompt_a_done = false;
                    if pressed_prompt_a {
                        machine.relacher(Machine::BOUTON_A);
                        pressed_prompt_a = false;
                    }
                    if pressed_prompt_b {
                        machine.relacher(Machine::BOUTON_B);
                        pressed_prompt_b = false;
                    }
                }
                // Skip the 3-unit intro timer and transition animations.
                // Preserve the confirmation screen's 30-unit timeout and
                // leave file parsing/authentication untouched.
                if scene == 0x3e9 {
                    machine.bus.sram.data[0xe0b0..0xe0b4]
                        .copy_from_slice(&600u32.to_le_bytes());
                }
                if matches!(scene, 0x3e9 | 0x3eb | 0x3ec) {
                    machine.bus.sram.data[index + 0x2a..index + 0x2c]
                        .copy_from_slice(&0u16.to_le_bytes());
                }
            }
        }
        if matches!(pc, 0x1000_5b80 | 0x1000_55b0 | 0x1000_2c60 | 0x1000_40d8 | 0x1000_411c) {
            eprintln!("scene checkpoint steps={n} pc={pc:#010x} r0={:#010x}", machine.cpu.regs.get_reg(0));
        }
        if pc == 0x1001_4a30 {
            eprintln!("f_open steps={n} path={:#010x} mode={:#x}",
                machine.cpu.regs.get_reg(1), machine.cpu.regs.get_reg(2));
        }
        if (80_000_000..82_000_000).contains(&n) && pc == 0x21bc && input_samples < 12 {
            eprintln!("input sample steps={n} id={:#x} p1={:#x} lr={:#x}",
                machine.cpu.regs.get_reg(0), machine.periph.port1.entrees, machine.cpu.regs.lr);
            input_samples += 1;
        }
        if matches!(pc, 0x1000_4dcc | 0x1000_5a8c | 0x1000_5708 | 0x1000_31ec | 0x1000_4e50 | 0x1000_3c98 | 0x1000_3d42 | 0x1000_3d46 | 0x1000_44b8) {
            eprintln!("update checkpoint steps={n} pc={pc:#010x} r0={:#010x}", machine.cpu.regs.get_reg(0));
        }
        if matches!(pc, 0x1000_51d0 | 0x1000_51f2 | 0x1000_520e | 0x1000_5224 | 0x1000_5258 | 0x1000_526a | 0x1000_5274 | 0x1000_5296 | 0x1000_529c | 0x1000_52d0 | 0x1000_531e) {
            eprintln!("update path steps={n} pc={pc:#010x} r0={:#010x}", machine.cpu.regs.get_reg(0));
        }
        if pc == 0x1000_529c && !released {
            for pin in [15, 14, 13] {
                machine.periph.port1.relacher(pin);
            }
            released = true;
        }
        if pc == Machine::SORTIE_CONSOLE {
            let c = (machine.cpu.regs.get_reg(0) & 0xff) as u8;
            if c == b'\n' {
                if direct_console.contains("update") || direct_console.contains("SD") {
                    eprintln!("console: {}", direct_console);
                }
                direct_console.clear();
            } else if c.is_ascii_graphic() || c == b' ' {
                direct_console.push(c as char);
            }
        }
        if (pc == 0x1000_9178 || pc == 0x1000_f3b0)
            && (machine.cpu.regs.get_reg(1) % 1024 == 0 || n < 170_000_000) {
            eprintln!("SD call steps={n} pc={pc:#010x} lba={} count={}",
                machine.cpu.regs.get_reg(1), machine.cpu.regs.get_reg(2));
        }
        if n % 1_000_000 == 0 {
            eprintln!("steps={n} pc={pc:#010x} sd_reads={}", machine.my_lab_card.as_ref().unwrap().read_count);
        }
        let reads = machine.my_lab_card.as_ref().unwrap().read_count;
        if reads != previous_reads {
            if reads / 1024 != previous_reads / 1024 || reads < 16 {
                eprintln!("SD read steps={n} pc={pc:#010x} sectors={reads}");
            }
            previous_reads = reads;
        }
        // The stock PRAM delay loop is CPU timing, not an SD protocol action.
        // Shorten it so a headless probe can reach update code in reasonable time.
        if pc == 0x4796 && machine.cpu.regs.get_reg(0) > 1 {
            machine.cpu.regs.set_reg(0, 1);
        }
        match machine.step() {
            StepResult::Ok(_) => {}
            _ => {
                eprintln!("stopped at {pc:#010x}");
                break;
            }
        }
        if machine.console.len() > previous_console {
            print!("{}", &machine.console[previous_console..]);
            previous_console = machine.console.len();
        }
    }
    if let Some(mut recorder) = recorder {
        recorder.update_frame(&lcd_frame(&machine)).expect("record final LCD frame");
        std::thread::sleep(std::time::Duration::from_secs(2));
        recorder.finish().expect("finish MP4 recording");
    }
    eprintln!("final sd reads={}", machine.my_lab_card.as_ref().unwrap().read_count);
    let status = u32::from_le_bytes(machine.bus.sram.data[0x1bfd4..0x1bfd8].try_into().unwrap());
    let detail = u32::from_le_bytes(machine.bus.sram.data[0x1bfd8..0x1bfdc].try_into().unwrap());
    eprintln!("final update status={status:#010x} detail={detail:#010x}");
    if let Some(path) = expected_rom {
        let expected = std::fs::read(path).expect("read expected firmware");
        let first = 0x100000;
        let last = 0xa6c000;
        let mismatches = machine.bus.flash.data[first..last].iter()
            .zip(&expected[first..last]).filter(|(a, b)| a != b).count();
        eprintln!("flash update-region mismatches={mismatches}");
    }
    let lcd = &machine.periph.display;
    eprintln!("display frames={} unique_colors={}", lcd.trames,
        lcd.vram.iter().copied().collect::<std::collections::BTreeSet<_>>().len());
    let mut picture = image::RgbImage::new(lcd.width as u32, lcd.height as u32);
    for (i, &pixel) in lcd.vram.iter().enumerate() {
        picture.put_pixel((i % lcd.width) as u32, (i / lcd.width) as u32, image::Rgb([
            (((pixel >> 11) & 31) * 255 / 31) as u8,
            (((pixel >> 5) & 63) * 255 / 63) as u8,
            ((pixel & 31) * 255 / 31) as u8,
        ]));
    }
    picture.save("/tmp/mylab-update-screen.png").expect("save screen");
}
