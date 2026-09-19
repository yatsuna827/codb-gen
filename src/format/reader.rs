use crate::format::{
    decode_count, rice_coding::RiceCodingReader, Header, HEADER_LEN, MAGIC, VERSION,
};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

pub(crate) struct CountsReader<'a> {
    reader: RiceCodingReader<'a>,
    k: u8,
}
impl<'a> CountsReader<'a> {
    pub(crate) fn new(buf: &'a [u8], k: u8) -> Self {
        Self {
            reader: RiceCodingReader::new(buf),
            k,
        }
    }

    #[inline(always)]
    pub(crate) fn read(&mut self) -> u32 {
        decode_count(self.reader.read(self.k))
    }
}

pub(crate) fn read_header(f: &mut File) -> Header {
    let mut h = [0u8; HEADER_LEN as usize];
    f.read_exact(&mut h).expect("read header");
    assert!(h[0..4] == MAGIC, "bad magic");
    let version = u32::from_le_bytes(h[4..8].try_into().unwrap());
    assert!(version == VERSION, "unsupported version {}", version);
    let u64at = |o: usize| u64::from_le_bytes(h[o..o + 8].try_into().unwrap());
    let off_seeds = u64at(8);
    let checksum = u64at(16);
    let rice_k = h[24];
    assert!(off_seeds >= HEADER_LEN, "bad seeds offset");
    assert!(
        (off_seeds - HEADER_LEN) % 8 == 0,
        "counts section not 8B-aligned"
    );
    let file_len = f.metadata().expect("meta").len();
    assert!(file_len >= off_seeds, "bad file length");
    assert!((file_len - off_seeds) % 2 == 0, "bad seeds length");
    let entry_count = (file_len - off_seeds) / 2;
    Header {
        entry_count,
        off_seeds,
        checksum,
        rice_k,
    }
}

pub(crate) fn read_at(f: &mut File, off: u64, buf: &mut [u8]) {
    f.seek(SeekFrom::Start(off)).expect("seek");
    f.read_exact(buf).expect("read");
}
