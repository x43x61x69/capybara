//! Prepare a private ROM for the H717 packer. Never include output in git.
use capybara::emulator::Machine;
use std::{fs, path::PathBuf};
fn main() -> Result<(),Box<dyn std::error::Error>> {
    let args:Vec<_>=std::env::args_os().skip(1).collect();
    if args.len()!=2 {return Err("usage: h717_export <own-dump.bin> <output-directory> (SONIX_DEVICE_KEY or .key sidecar)".into());}
    let dump=PathBuf::from(&args[0]);let output=PathBuf::from(&args[1]);
    let mut machine=Machine::new();
    machine.device_key=capybara::emulator::loader::resolve_device_key(&dump);
    let report=machine.load_firmware_file(&dump)?;
    if !report.bootable {return Err("ROM cannot boot; supply its device key through SONIX_DEVICE_KEY or its .key sidecar".into());}
    fs::create_dir_all(&output)?;
    fs::write(output.join("flash.bin"),&machine.bus.flash.data)?;
    fs::write(output.join("pram.bin"),&machine.bus.pram.data)?;
    fs::write(output.join("mailbox.bin"),&machine.bus.sram.mailbox)?;
    // The mailbox's boot information is authoritative even if XIP has not
    // been enabled by the guest yet. Its register will be programmed at boot.
    let base=u32::from_le_bytes(machine.bus.sram.mailbox[0x818..0x81c].try_into().unwrap());
    fs::write(output.join("metadata.json"),serde_json::to_vec_pretty(&serde_json::json!({
        "device_key":machine.device_key.or(machine.periph.fuses.device_key),"xip_base":base,
    }))?)?;
    println!("Prepared private image in {}",output.display());Ok(())
}
