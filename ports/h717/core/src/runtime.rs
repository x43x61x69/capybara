use alloc::{boxed::Box, collections::{BTreeMap, VecDeque}, vec::Vec};
use core::{ffi::c_void, ptr, slice};
use crate::{emulator::{cpu::{Cpu, StepResult}, mmu::MemoryBus, peripherals::Peripherals}, flash::{crc32, Reader, Read, PAGE, MAX_SAVE_PAGES}};

#[repr(C)]
#[derive(Default, Debug)]
pub struct H717Stats {
    pub cycles: u64, pub instructions: u64, pub recompiled_instructions: u64,
    pub pc: u32, pub frames: u32, pub baud: u32, pub sleeping: u32, pub error: u32,
}
pub struct H717Machine {
    pub cpu: Cpu, pub bus: MemoryBus, pub periph: Peripherals,
    instructions: u64, stop: u32, buttons: u8,
    dial: VecDeque<(bool,bool)>, dial_due: u64,
    wall: Option<(u32,u32)>, shown_enabled: Option<bool>,
    #[cfg(feature = "recomp")]
    recomp_enabled: bool,
    #[cfg(feature = "recomp")]
    recompiled_instructions: u64,
}
impl H717Machine {
    #[cfg(feature = "recomp")]
    pub fn set_recomp_enabled(&mut self, enabled: bool) {
        self.recomp_enabled = enabled;
    }
    fn sleeping(&self) -> bool { self.periph.pmu.deep_sleep_active || self.periph.snsys.power_down_active() }
    fn reset(&mut self) {
        self.cpu.reset(&mut self.bus,&mut self.periph);
        self.periph.uart.reinitialiser(); self.dial_due=0;
    }
    fn wake(&mut self) {
        if self.sleeping() {
            self.periph.snsys.declencher_reveil(); self.periph.pmu.declencher_reveil_broche();
            self.periph.snsys.reveil_demande=false; self.reset();
        }
    }
    fn tick_dial(&mut self) {
        if self.cpu.cycles < self.dial_due { return; }
        if let Some((a,b))=self.dial.pop_front() {
            if a {self.periph.port2.relacher(0);} else {self.periph.port2.appuyer(0);}
            if b {self.periph.port2.relacher(1);} else {self.periph.port2.appuyer(1);}
            self.dial_due=self.cpu.cycles+1_920_000;
        }
    }
}
fn word(data:&[u8],offset:usize)->u32 {u32::from_le_bytes(data[offset..offset+4].try_into().unwrap())}
fn put(data:&mut[u8],offset:usize,value:u32) {data[offset..offset+4].copy_from_slice(&value.to_le_bytes());}

/// The callback and context must remain valid, and must fill the whole buffer
/// on success. A machine and its buffers may only be used by one task at a time.
#[no_mangle]
pub unsafe extern "C" fn h717_create_reader(read: Option<Read>, context:*mut c_void, length:usize) -> *mut H717Machine {
    let Some(read)=read else {return ptr::null_mut();};
    if length<64 || length>12*1024*1024 {return ptr::null_mut();}
    let reader=Reader {read,context,length};
    let mut header=[0u8;64];
    if !reader.read_into(0,&mut header) {return ptr::null_mut();}
    let mut bus=MemoryBus::default();
    if !bus.flash.attach(reader,&header) {return ptr::null_mut();}
    if !reader.read_into(word(&header,36) as usize,&mut bus.pram.data)
        || !reader.read_into(word(&header,40) as usize,&mut bus.sram.mailbox)
        || crc32(&bus.pram.data)!=word(&header,56)
        || crc32(&bus.sram.mailbox)!=word(&header,60) {return ptr::null_mut();}
    let sp=bus.pram.read_u32(0); let pc=bus.pram.read_u32(4);
    if !(0x1800_0000..=0x1802_0000).contains(&sp) || pc&1==0 || pc&!1>=65536 {return ptr::null_mut();}
    bus.pram.loaded=true;
    let mut periph=Peripherals::default();
    periph.fuses.device_key=Some(word(&header,20));
    periph.xip.base=word(&header,24);
    let mut cpu=Cpu::default(); cpu.reset(&mut bus,&mut periph);
    Box::into_raw(Box::new(H717Machine {cpu,bus,periph,instructions:0,stop:0,buttons:0,
        dial:VecDeque::new(),dial_due:0,wall:None,shown_enabled:None,
        #[cfg(feature = "recomp")]
        recomp_enabled: true,
        #[cfg(feature = "recomp")]
        recompiled_instructions: 0,
    }))
}
#[no_mangle]
pub unsafe extern "C" fn h717_destroy(m:*mut H717Machine) {if !m.is_null(){drop(Box::from_raw(m));}}
#[no_mangle]
#[cfg_attr(target_arch = "xtensa", link_section = ".iram1.h717_step")]
pub unsafe extern "C" fn h717_step(m:*mut H717Machine,budget:u32)->i32 {
    let Some(m)=m.as_mut() else {return 3;};
    if m.stop!=0 {return m.stop as i32;}
    let budget = budget.min(100_000);
    let mut completed = 0;
    while completed < budget {
        // A batch can start at any of the three instructions in the loop.
        // Leave room for pending UART bytes and keep the external I/O cadence.
        if completed < 3 && m.dial.is_empty()
            && m.bus.flash.error.get() == 0 && m.periph.uart.tx_out.len() < 8192 - 32
        {
            let advanced = m.cpu.step_countdown(&mut m.bus, &mut m.periph, (budget - completed).min(512));
            completed += advanced;
            m.instructions += u64::from(advanced);
            if completed == budget { break; }
        }
        if m.periph.snsys.reveil_demande {
            m.periph.snsys.reveil_demande=false;
            if m.sleeping(){m.periph.pmu.declencher_reveil_rtc();m.reset();}
        }
        m.tick_dial();
        #[cfg(feature = "recomp")]
        if m.recomp_enabled && m.dial.is_empty() {
            let advanced = crate::recomp::step(&mut m.cpu, &mut m.bus, &mut m.periph, budget - completed);
            if advanced != 0 {
                completed += advanced;
                m.instructions += u64::from(advanced);
                m.recompiled_instructions += u64::from(advanced);
                if m.bus.flash.error.get() != 0 || m.periph.uart.tx_out.len() > 8192 {
                    m.stop = 3;
                    break;
                }
                continue;
            }
        }
        match m.cpu.step(&mut m.bus,&mut m.periph) {
            StepResult::Ok(_) => m.instructions+=1,
            StepResult::Undefined(_) => {m.stop=2;break;},
            _ => {m.stop=1;break;},
        }
        completed += 1;
        if m.bus.flash.error.get()!=0 || m.periph.uart.tx_out.len()>8192 {m.stop=3;break;}
    }
    m.stop as i32
}
#[no_mangle]
pub unsafe extern "C" fn h717_buttons(m:*mut H717Machine,mask:u8) {
    let Some(m)=m.as_mut() else {return;};
    if mask & !m.buttons !=0 {m.wake();}
    m.buttons=mask&15;
    for (bit,pin) in [9,11,10,8].iter().enumerate() {
        if mask&(1<<bit)!=0 {m.periph.port0.appuyer(*pin);} else {m.periph.port0.relacher(*pin);}
    }
}
#[no_mangle]
pub unsafe extern "C" fn h717_dial(m:*mut H717Machine,delta:i32) {
    let Some(m)=m.as_mut() else {return;}; if delta==0 {return;} m.wake();
    for _ in 0..delta.unsigned_abs().min(16) {
        if m.dial.len()>60 {break;}
        for (a,b) in [(false,true),(false,false),(true,false),(true,true)] {
            m.dial.push_back(if delta>0 {(a,b)} else {(b,a)});
        }
    }
}
#[no_mangle]
pub unsafe extern "C" fn h717_frame(m:*mut H717Machine,out:*mut u16)->bool {
    let Some(m)=m.as_mut() else {return false;}; if out.is_null(){return false;}
    let d=&mut m.periph.display;
    if !d.dirty && m.shown_enabled==Some(d.is_enabled){return false;}
    let frame=slice::from_raw_parts_mut(out,128*128);
    if d.is_enabled {frame.copy_from_slice(&d.vram[..128*128]);} else {frame.fill(0);}
    d.dirty=false; m.shown_enabled=Some(d.is_enabled);true
}
#[no_mangle]
pub unsafe extern "C" fn h717_uart_rx(m:*mut H717Machine,data:*const u8,len:usize)->usize {
    let Some(m)=m.as_mut() else {return 0;};
    if data.is_null() {return 0;}
    // A disabled physical receiver drops line noise; do not replay it later.
    if m.periph.uart.ctrl&0x41!=0x41 {return len;}
    let n=len.min(8192usize.saturating_sub(m.periph.uart.rx_in.len()));
    m.periph.uart.inject_rx_bytes(slice::from_raw_parts(data,n)); n
}
#[no_mangle]
pub unsafe extern "C" fn h717_uart_tx(m:*mut H717Machine,out:*mut u8,capacity:usize)->usize {
    let Some(m)=m.as_mut() else {return 0;}; if out.is_null(){return 0;}
    let n=capacity.min(m.periph.uart.tx_out.len());
    for byte in slice::from_raw_parts_mut(out,n){*byte=m.periph.uart.tx_out.pop_front().unwrap();} n
}
#[no_mangle]
pub unsafe extern "C" fn h717_stats(m:*mut H717Machine,out:*mut H717Stats) {
    let (Some(m),Some(out))=(m.as_ref(),out.as_mut()) else {return;};
    *out=H717Stats {cycles:m.cpu.cycles,instructions:m.instructions,
        #[cfg(feature = "recomp")]
        recompiled_instructions:m.recompiled_instructions,
        #[cfg(not(feature = "recomp"))]
        recompiled_instructions:0,
        pc:m.cpu.regs.pc,
        frames:m.periph.display.trames as u32,
        baud:if m.periph.uart.ctrl&1!=0 && m.periph.uart.ctrl&0xC0!=0 {m.periph.uart.baud_rate(96_000_000)}else{0},
        sleeping:m.sleeping() as u32,error:if m.bus.flash.error.get()!=0 {0x100+m.bus.flash.error.get()}else{m.stop}};
}
#[no_mangle]
pub unsafe extern "C" fn h717_clock(m:*mut H717Machine,seconds:u32) {
    let Some(m)=m.as_mut() else {return;};
    let (host,rtc)=*m.wall.get_or_insert((seconds,m.periph.snsys.secondes));
    let target=rtc.wrapping_add(seconds.wrapping_sub(host));
    let delta=target.wrapping_sub(m.periph.snsys.secondes);
    if delta>0 && delta<0x8000_0000 {
        let mut cycles=delta as u64*96_000_000;
        while cycles>0 {let n=cycles.min(u32::MAX as u64) as u32;m.periph.snsys.tick(n);cycles-=n as u64;}
    }
}
#[no_mangle]
pub unsafe extern "C" fn h717_save(m:*mut H717Machine,out:*mut u8,capacity:usize)->usize {
    let Some(m)=m.as_ref() else {return 0;};
    let regs=m.periph.snsys.registres();
    let needed=32+regs.len()*8+m.bus.flash.pages.len()*(PAGE+4);
    if out.is_null(){return needed;} if capacity<needed{return 0;}
    let data=slice::from_raw_parts_mut(out,needed); data.fill(0);
    data[..8].copy_from_slice(b"H717SAV1");put(data,8,m.bus.flash.identity);
    put(data,12,m.periph.snsys.secondes);put(data,16,regs.len() as u32);
    put(data,20,m.bus.flash.pages.len() as u32);put(data,24,needed as u32);
    let mut cursor=32;
    for (key,value) in regs {put(data,cursor,key);put(data,cursor+4,value);cursor+=8;}
    for (page,bytes) in &m.bus.flash.pages {
        put(data,cursor,*page as u32);cursor+=4;
        data[cursor..cursor+PAGE].copy_from_slice(bytes.as_slice());cursor+=PAGE;
    }
    // Include identity, clock and sizes in the checksum, not only payload.
    let checksum=crc32(&data[..28])^crc32(&data[32..]);put(data,28,checksum);needed
}
#[no_mangle]
pub unsafe extern "C" fn h717_restore_save(m:*mut H717Machine,source:*const u8,length:usize)->bool {
    let Some(m)=m.as_mut() else {return false;};
    if source.is_null() || length<32 || length>1024*1024 || m.instructions!=0 {return false;}
    let data=slice::from_raw_parts(source,length);
    if &data[..8]!=b"H717SAV1" || word(data,8)!=m.bus.flash.identity
        || word(data,24) as usize!=length || word(data,28)!=(crc32(&data[..28])^crc32(&data[32..])) {return false;}
    let reg_count=word(data,16) as usize;let page_count=word(data,20) as usize;
    if reg_count>256 || page_count>MAX_SAVE_PAGES || 32+reg_count*8+page_count*(PAGE+4)!=length {return false;}
    let mut cursor=32+reg_count*8;let mut previous=None;
    // Validate all page indices before mutating the machine.
    for _ in 0..page_count {
        let page=word(data,cursor) as usize;
        if page>=4096 || previous.is_some_and(|p|page<=p){return false;}
        previous=Some(page);cursor+=PAGE+4;
    }
    let mut regs=Vec::with_capacity(reg_count);cursor=32;
    for _ in 0..reg_count {regs.push((word(data,cursor),word(data,cursor+4)));cursor+=8;}
    let mut pages=BTreeMap::new();
    for _ in 0..page_count {
        let page=word(data,cursor) as usize;cursor+=4;
        let bytes:Box<[u8;PAGE]>=data[cursor..cursor+PAGE].to_vec().into_boxed_slice().try_into().unwrap();
        pages.insert(page,bytes);cursor+=PAGE;
    }
    m.bus.flash.pages=pages;m.periph.snsys.poser_registres(&regs);
    m.periph.snsys.secondes=word(data,12);m.wall=None;true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flash::ROM_SIZE;
    use alloc::vec;
    unsafe extern "C" fn reader(context:*mut c_void,offset:u32,out:*mut u8,length:usize)->bool {
        let bytes=&*(context as *const Vec<u8>);
        let Some(part)=bytes.get(offset as usize..offset as usize+length) else {return false;};
        ptr::copy_nonoverlapping(part.as_ptr(),out,length);true
    }
    fn package()->Vec<u8> {
        let pram=64;let mailbox=pram+65536;let index=mailbox+PAGE;let payload=index+4096*12;
        let mut bytes=vec![0;payload+PAGE];
        bytes[..8].copy_from_slice(b"H717ROM1");put(&mut bytes,8,1);
        let len=bytes.len();put(&mut bytes,12,len as u32);put(&mut bytes,16,1234);
        put(&mut bytes,24,0x60011000);put(&mut bytes,28,PAGE as u32);put(&mut bytes,32,4096);
        put(&mut bytes,36,pram as u32);put(&mut bytes,40,mailbox as u32);
        put(&mut bytes,44,index as u32);put(&mut bytes,48,payload as u32);
        put(&mut bytes,pram,0x18020000);put(&mut bytes,pram+4,0x101);
        bytes[pram+0x100..pram+0x104].copy_from_slice(&[7,0x20,0xfe,0xe7]); // MOVS r0,7; B .
        bytes[payload..].fill(0xff);
        let page_crc=crc32(&bytes[payload..]);
        for page in 0..ROM_SIZE/PAGE {
            put(&mut bytes,index+page*12,payload as u32);put(&mut bytes,index+page*12+4,PAGE as u32);
            put(&mut bytes,index+page*12+8,page_crc);
        }
        let crc=crc32(&bytes[..52]);put(&mut bytes,52,crc);
        let crc=crc32(&bytes[pram..mailbox]);put(&mut bytes,56,crc);
        let crc=crc32(&bytes[mailbox..index]);put(&mut bytes,60,crc);bytes
    }
    unsafe fn create(bytes:&mut Vec<u8>)->*mut H717Machine {
        h717_create_reader(Some(reader),bytes as *mut _ as *mut c_void,bytes.len())
    }
    #[test]
    fn countdown_preserves_interpreter_state_across_peripheral_ticks() {
        unsafe {
            let mut bytes = package();
            let fast = create(&mut bytes);
            let slow = create(&mut bytes);
            for machine in [&mut *fast, &mut *slow] {
                machine.bus.pram.data[0x100..0x108]
                    .copy_from_slice(&[0x40, 0x1e, 0x00, 0xbf, 0xfc, 0xd5, 0xfe, 0xe7]);
                machine.cpu.regs.r[0] = 300;
            }
            for budget in [512, 257, 137] {
                assert_eq!(h717_step(fast, budget), 0);
                for _ in 0..budget {
                    assert_eq!(h717_step(slow, 1), 0);
                }
                assert_eq!((*fast).cpu.regs.r, (*slow).cpu.regs.r);
                assert_eq!((*fast).cpu.regs.xpsr, (*slow).cpu.regs.xpsr);
                assert_eq!((*fast).cpu.regs.pc, (*slow).cpu.regs.pc);
                assert_eq!((*fast).cpu.cycles, (*slow).cpu.cycles);
                assert_eq!((*fast).instructions, (*slow).instructions);
            }
            h717_destroy(fast);
            h717_destroy(slow);
        }
    }
    #[test] fn real_instruction_core_buttons_sleep_and_uart_fifo() {unsafe{
        let mut bytes=package();let m=create(&mut bytes);assert!(!m.is_null());
        assert_eq!(h717_step(m,32),0);assert_eq!((*m).cpu.regs.r[0],7);
        (*m).periph.snsys.write_reg(0x300,1);assert!((*m).sleeping());
        h717_buttons(m,5);assert!(!(*m).sleeping());
        assert_eq!((*m).periph.port0.entrees&((1<<9)|(1<<10)),0);
        h717_buttons(m,0);assert_ne!((*m).periph.port0.entrees&(1<<9),0);
        let packet=[0,0xff,0x42,0x13];assert_eq!(h717_uart_rx(m,packet.as_ptr(),4),4);
        (*m).periph.uart.tick(96000,96000000);
        let got:Vec<_>=(0..4).map(|_|(*m).periph.uart.read_reg(0) as u8).collect();
        assert_eq!(got,packet);
        (*m).periph.uart.write_reg(0,0xa5);(*m).periph.uart.tick(96000,96000000);
        let mut out=[0;8];assert_eq!(h717_uart_tx(m,out.as_mut_ptr(),8),1);assert_eq!(out[0],0xa5);
        h717_destroy(m);
    }}
    #[test] fn copy_on_write_save_identity_crc_and_restore_before_execution() {unsafe{
        let mut bytes=package();let m=create(&mut bytes);assert!(!m.is_null());
        (*m).bus.flash.write_u8(PAGE+13,0x42);(*m).periph.snsys.secondes=700;
        assert_eq!((*m).bus.flash.read_u8(13),0xff);
        assert_eq!((*m).bus.flash.read_u8(PAGE+13),0x42);
        let size=h717_save(m,ptr::null_mut(),0);let mut save=vec![0;size];
        assert_eq!(h717_save(m,save.as_mut_ptr(),size-1),0);
        assert_eq!(h717_save(m,save.as_mut_ptr(),size),size);
        let fresh=create(&mut bytes);save[size-1]^=1;
        assert!(!h717_restore_save(fresh,save.as_ptr(),size));assert!((*fresh).bus.flash.pages.is_empty());
        save[size-1]^=1;assert!(h717_restore_save(fresh,save.as_ptr(),size));
        assert_eq!((*fresh).bus.flash.read_u8(PAGE+13),0x42);assert_eq!((*fresh).periph.snsys.secondes,700);
        h717_clock(fresh,100);h717_clock(fresh,102);assert_eq!((*fresh).periph.snsys.secondes,702);
        assert_eq!(h717_step(fresh,1),0);assert!(!h717_restore_save(fresh,save.as_ptr(),size));
        (*m).bus.flash.identity=5678;assert!(!h717_restore_save(m,save.as_ptr(),size));
        h717_destroy(m);h717_destroy(fresh);
    }}
    #[test] fn bad_boot_payload_and_bad_rom_page_are_rejected() {unsafe{
        let mut bytes=package();bytes[64+300]^=1;assert!(create(&mut bytes).is_null());
        bytes[64+300]^=1;let m=create(&mut bytes);assert!(!m.is_null());
        let last=bytes.len()-1;bytes[last]^=1;
        assert_eq!((*m).bus.flash.read_u8(0),0xff);assert_eq!((*m).bus.flash.error.get(),1);
        assert_eq!(h717_step(m,1),3);h717_destroy(m);
    }}
}
