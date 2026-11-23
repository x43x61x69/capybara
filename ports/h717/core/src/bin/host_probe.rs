//! Exercise the actual embedded core/ROM codec on the host before flashing.
use h717_core::*;
use std::{ffi::c_void,fs,path::PathBuf,time::Instant};
unsafe extern "C" fn read(context:*mut c_void,offset:u32,out:*mut u8,length:usize)->bool {
    let data=&*(context as *const Vec<u8>);
    let Some(bytes)=data.get(offset as usize..offset as usize+length) else{return false;};
    std::ptr::copy_nonoverlapping(bytes.as_ptr(),out,length);true
}
fn main()->Result<(),Box<dyn std::error::Error>> {
    let args:Vec<_>=std::env::args().skip(1).collect();
    if args.is_empty(){return Err("usage: host_probe <tamarom.bin> [instructions=5000000] [frame.ppm]".into());}
    let mut data=fs::read(&args[0])?;
    let budget:u64=args.get(1).map(|v|v.parse()).transpose()?.unwrap_or(5_000_000);
    unsafe {
        let m=h717_create_reader(Some(read),&mut data as *mut _ as *mut c_void,data.len());
        if m.is_null(){return Err("embedded core rejected ROM package".into());}
        let start=Instant::now();let mut done=0;
        while done<budget {let n=(budget-done).min(512) as u32;if h717_step(m,n)!=0 {break;}done+=n as u64;}
        let mut stats=H717Stats::default();h717_stats(m,&mut stats);
        println!("{stats:?}; elapsed={:.3}s, instructions/s={:.0}",start.elapsed().as_secs_f64(),stats.instructions as f64/start.elapsed().as_secs_f64());
        let mut frame=vec![0u16;128*128];h717_frame(m,frame.as_mut_ptr());
        if let Some(path)=args.get(2) {
            let mut ppm=b"P6\n128 128\n255\n".to_vec();
            for pixel in frame {ppm.extend_from_slice(&[(((pixel>>11)&31)*255/31) as u8,(((pixel>>5)&63)*255/63) as u8,((pixel&31)*255/31) as u8]);}
            fs::write(PathBuf::from(path),ppm)?;
        }
        // Persist/restore flash+RTC through the public ABI on a fresh machine.
        let mut save=vec![0;h717_save(m,std::ptr::null_mut(),0)];
        assert_eq!(h717_save(m,save.as_mut_ptr(),save.len()),save.len());
        let fresh=h717_create_reader(Some(read),&mut data as *mut _ as *mut c_void,data.len());
        assert!(!fresh.is_null());assert!(h717_restore_save(fresh,save.as_ptr(),save.len()));
        h717_destroy(fresh);h717_destroy(m);
        if stats.error!=0{return Err(format!("emulation stopped, error {}",stats.error).into());}
    } Ok(())
}
