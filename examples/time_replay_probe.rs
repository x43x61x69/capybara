//! Replay clock-menu inputs on a copy of a snapshot, tracing system writes.
//! time_replay_probe <snapshot> <output prefix> [A,B,C,W,wait,holdC,...]
use capybara::emulator::{etat::Instantane, Machine, StepResult};

fn run(machine: &mut Machine, seconds: f64) {
    let end = machine.cpu.cycles + (seconds * 96_000_000.0) as u64;
    let watch = std::env::var("WATCH").ok().map(|s| u32::from_str_radix(&s, 16).unwrap());
    let mut history = std::collections::VecDeque::new();
    while machine.cpu.cycles < end {
        let before = watch.map(|address| u32::from_le_bytes(machine.bus.sram.data[(address - 0x18000000) as usize..(address - 0x18000000) as usize + 4].try_into().unwrap()));
        if watch.is_some() {
            history.push_back(machine.cpu.regs.pc);
            if history.len() > 40 { history.pop_front(); }
        }
        assert!(matches!(machine.step(), StepResult::Ok(_)), "stopped at {:x}", machine.cpu.regs.pc);
        if let Some(address) = watch {
            let after = u32::from_le_bytes(machine.bus.sram.data[(address - 0x18000000) as usize..(address - 0x18000000) as usize + 4].try_into().unwrap());
            if before != Some(after) {
                println!("watch {address:x}: {:x?} -> {after:x}, regs={:x?}", before, machine.cpu.regs.r);
                for &pc in &history {
                    let instruction = &machine.get_disassembly_at(pc, 1)[0];
                    println!("{pc:08x} {} {}", instruction.mnemonic, instruction.operands);
                }
            }
        }
    }
}

fn picture(machine: &Machine, path: &str) {
    let mut image = image::RgbImage::new(128, 128);
    for (i, p) in machine.periph.display.vram.iter().enumerate() {
        image.put_pixel((i % 128) as u32, (i / 128) as u32, image::Rgb([
            (((p >> 11) & 31) * 255 / 31) as u8,
            (((p >> 5) & 63) * 255 / 63) as u8,
            ((p & 31) * 255 / 31) as u8,
        ]));
    }
    image.save(path).unwrap();
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let state = Instantane::lire(std::path::Path::new(&args[1])).unwrap();
    let mut machine = Machine::new();
    machine.device_key = state.periph.fuses.device_key;
    assert!(machine.load_firmware_file(&state.firmware).unwrap().bootable);
    machine.restaurer(&state);
    machine.bus.mmio_trace.enabled = true;
    machine.bus.mmio_trace.log_page = Some(0x45000000);
    machine.bus.mmio_trace.log_ecritures_seules = true;
    picture(&machine, &format!("{}-0.png", args[2]));
    for (i, action) in args.get(3).map(String::as_str).unwrap_or("wait").split(',').enumerate() {
        let button = match action.trim_start_matches("hold") {
            "A" => Some(Machine::BOUTON_A), "B" => Some(Machine::BOUTON_B),
            "C" => Some(Machine::BOUTON_C), "W" => Some(Machine::BOUTON_MOLETTE),
            "wait" => None, _ => panic!("unknown action {action}"),
        };
        if let Some(button) = button { machine.appuyer(button); }
        run(&mut machine, if action.starts_with("hold") { 3.5 } else { 0.2 });
        if let Some(button) = button { machine.relacher(button); }
        run(&mut machine, 0.4);
        picture(&machine, &format!("{}-{}.png", args[2], i + 1));
        println!("{} {action} seconds={}", i + 1, machine.periph.snsys.secondes);
    }
    for entry in &machine.bus.mmio_trace.log {
        if entry.addr >= 0x45000200 {
            println!("pc={:08x} write {:08x}={:08x}", entry.pc, entry.addr, entry.value);
        }
    }
    for offset in (0..machine.bus.sram.data.len() - 12).step_by(2) {
        let data = &machine.bus.sram.data[offset..offset+12];
        let fields: Vec<_> = data.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
        if (2020..2100).contains(&fields[0]) && (1..=12).contains(&fields[1])
            && (1..=31).contains(&fields[2]) && fields[3] < 24 && fields[4] < 60 && fields[5] < 60 {
            println!("calendar {:08x} {fields:?}", 0x18000000 + offset);
        }
    }
    machine.instantane().ecrire(std::path::Path::new(&format!("{}.tamastate", args[2]))).unwrap();
}
