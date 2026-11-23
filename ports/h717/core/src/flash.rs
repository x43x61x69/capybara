//! Bounded, copy-on-write flash backed by individually compressed ROM pages.
use alloc::{boxed::Box, collections::BTreeMap, vec, vec::Vec};
use core::cell::{Cell, RefCell};
use core::ffi::c_void;

pub const PAGE: usize = 4096;
pub const ROM_SIZE: usize = 16 * 1024 * 1024;
const CACHE_SLOTS: usize = 256;
pub const MAX_SAVE_PAGES: usize = 64;
pub type Read = unsafe extern "C" fn(*mut c_void, u32, *mut u8, usize) -> bool;

#[derive(Clone, Copy)]
pub struct Reader { pub read: Read, pub context: *mut c_void, pub length: usize }
impl Reader {
    pub fn read_into(&self, offset: usize, out: &mut [u8]) -> bool {
        if offset.checked_add(out.len()).map_or(true, |end| end > self.length) { return false; }
        unsafe { (self.read)(self.context, offset as u32, out.as_mut_ptr(), out.len()) }
    }
}
#[derive(Clone, Copy)]
struct Entry { offset: usize, length: usize, crc: u32 }
struct Cache { bytes: Vec<u8>, tags: [usize; CACHE_SLOTS], scratch: [u8; PAGE] }

pub struct SpiFlash {
    pub size: usize,
    pub revision: u64,
    pub identity: u32,
    pub pages: BTreeMap<usize, Box<[u8; PAGE]>>,
    reader: Option<Reader>,
    entries: Vec<Entry>,
    cache: RefCell<Cache>,
    pub error: Cell<u32>,
}
impl Default for SpiFlash {
    fn default() -> Self {
        Self { size: ROM_SIZE, revision: 0, identity: 0, pages: BTreeMap::new(), reader: None,
            entries: Vec::new(), error: Cell::new(0), cache: RefCell::new(Cache {
                bytes: vec![0; CACHE_SLOTS * PAGE], tags: [usize::MAX; CACHE_SLOTS], scratch: [0; PAGE],
            }) }
    }
}
impl SpiFlash {
    pub fn attach(&mut self, reader: Reader, header: &[u8; 64]) -> bool {
        let word = |offset| u32::from_le_bytes(header[offset..offset+4].try_into().unwrap());
        if &header[..8] != b"H717ROM1" || word(8) != 1 || word(28) != PAGE as u32
            || word(32) != (ROM_SIZE/PAGE) as u32 || word(12) as usize != reader.length
            || crc32(&header[..52]) != word(52) || reader.length > 12*1024*1024
        { return false; }
        let pram = word(36) as usize;
        let mailbox = word(40) as usize;
        let index = word(44) as usize;
        let data = word(48) as usize;
        if pram < 64 || pram.checked_add(65536) != Some(mailbox)
            || mailbox.checked_add(PAGE) != Some(index)
            || index.checked_add(ROM_SIZE/PAGE*12) != Some(data)
            || data > reader.length { return false; }
        let mut entries = Vec::with_capacity(ROM_SIZE/PAGE);
        let mut record = [0u8; 12];
        for page in 0..ROM_SIZE/PAGE {
            if !reader.read_into(index + page*12, &mut record) { return false; }
            let offset = u32::from_le_bytes(record[..4].try_into().unwrap()) as usize;
            let length = u32::from_le_bytes(record[4..8].try_into().unwrap()) as usize;
            if length == 0 || length > PAGE || offset < data
                || offset.checked_add(length).map_or(true, |end| end > reader.length) { return false; }
            entries.push(Entry { offset, length, crc: u32::from_le_bytes(record[8..].try_into().unwrap()) });
        }
        self.identity = word(16);
        self.entries = entries;
        self.reader = Some(reader);
        true
    }

    fn read_word(&self, offset: usize, count: usize) -> u32 {
        if offset.checked_add(count).map_or(true, |end| end > self.size) { return u32::MAX; }
        if offset/PAGE != (offset+count-1)/PAGE {
            return (0..count).fold(0, |v,k| v | ((self.read_u8(offset+k) as u32) << (k*8)));
        }
        let page = offset/PAGE;
        let within = offset%PAGE;
        if let Some(bytes) = self.pages.get(&page) {
            return (0..count).fold(0, |v,k| v | ((bytes[within+k] as u32) << (k*8)));
        }
        let Some(reader) = self.reader else { return u32::MAX; };
        let mut cache = self.cache.borrow_mut();
        let slot = page % CACHE_SLOTS;
        let start = slot*PAGE;
        if cache.tags[slot] != page {
            let entry = self.entries[page];
            let Cache { scratch, bytes, tags } = &mut *cache;
            let out = &mut bytes[start..start+PAGE];
            let read_ok = if entry.length == PAGE { reader.read_into(entry.offset, out) }
                else { reader.read_into(entry.offset, &mut scratch[..entry.length])
                    && decode_lz4(&scratch[..entry.length], out) };
            if !read_ok || crc32(out) != entry.crc {
                self.error.set(1); out.fill(0xFF); return u32::MAX;
            }
            tags[slot] = page;
        }
        (0..count).fold(0, |v,k| v | ((cache.bytes[start+within+k] as u32) << (k*8)))
    }
    #[inline] pub fn read_u8(&self, offset: usize) -> u8 { self.read_word(offset,1) as u8 }
    #[inline] pub fn read_u16(&self, offset: usize) -> u16 { self.read_word(offset,2) as u16 }
    #[inline] pub fn read_u32(&self, offset: usize) -> u32 { self.read_word(offset,4) }
    pub fn write_u8(&mut self, offset: usize, value: u8) {
        if offset >= self.size || self.read_u8(offset) == value { return; }
        let page = offset/PAGE;
        if !self.pages.contains_key(&page) {
            if self.pages.len() >= MAX_SAVE_PAGES { self.error.set(2); return; }
            let mut data = vec![0;PAGE].into_boxed_slice();
            for (index, byte) in data.iter_mut().enumerate() { *byte = self.read_u8(page*PAGE+index); }
            self.pages.insert(page, data.try_into().unwrap());
        }
        self.pages.get_mut(&page).unwrap()[offset%PAGE] = value;
        self.revision = self.revision.wrapping_add(1);
    }
}

/// Standard LZ4 block; every bounds check precedes the corresponding copy.
fn decode_lz4(src: &[u8], out: &mut [u8]) -> bool {
    let (mut input, mut output) = (0usize,0usize);
    fn length(src: &[u8], cursor: &mut usize, mut value: usize) -> Option<usize> {
        if value == 15 { loop {
            let next = *src.get(*cursor)? as usize; *cursor += 1;
            value = value.checked_add(next)?; if next != 255 { break; }
        }}
        Some(value)
    }
    while input < src.len() {
        let token = src[input]; input += 1;
        let Some(literals) = length(src,&mut input,(token>>4) as usize) else { return false; };
        if literals > src.len()-input || literals > out.len()-output { return false; }
        out[output..output+literals].copy_from_slice(&src[input..input+literals]);
        input += literals; output += literals;
        if input == src.len() { return output == out.len(); }
        if input+2 > src.len() { return false; }
        let distance = u16::from_le_bytes([src[input],src[input+1]]) as usize; input += 2;
        let Some(matches) = length(src,&mut input,(token&15) as usize).and_then(|v|v.checked_add(4)) else { return false; };
        if distance == 0 || distance > output || matches > out.len()-output { return false; }
        for k in 0..matches { out[output+k] = out[output+k-distance]; }
        output += matches;
    }
    output == out.len()
}
const fn crc_table() -> [u32;256] {
    let mut table = [0;256]; let mut i=0;
    while i<256 { let mut crc=i as u32; let mut bit=0;
        while bit<8 { crc=if crc&1!=0 { (crc>>1)^0xEDB88320 } else {crc>>1}; bit+=1; }
        table[i]=crc; i+=1;
    } table
}
pub fn crc32(bytes: &[u8]) -> u32 {
    const TABLE:[u32;256]=crc_table();
    let mut crc=!0u32;
    for byte in bytes { crc=TABLE[((crc as u8)^byte) as usize]^(crc>>8); } !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn lz4_overlap_and_truncation() {
        let data=[0x1f,b'A',1,0,255,255,255,255,255,255,255,255,255,255,255,255,255,255,255,251];
        // Prefer compact, independently known block: one literal + 19 matches.
        assert!(decode_lz4(&[0x1f,b'A',1,0,0],&mut [0;20]));
        assert!(!decode_lz4(&[0x1f,b'A',1,0],&mut [0;20]));
        assert!(!decode_lz4(&[0x00,0,0],&mut [0;4]));
        let mut full=[0;PAGE];
        assert!(decode_lz4(&data,&mut full));
        assert!(full.iter().all(|v|*v==b'A'));
        assert_eq!(crc32(b"123456789"),0xcbf43926);
    }
}
