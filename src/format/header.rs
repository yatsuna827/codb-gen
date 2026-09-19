pub(crate) const MAGIC: [u8; 4] = *b"CODB";
pub(crate) const VERSION: u32 = 1;
pub(crate) const P_COUNT: usize = 24usize.pow(5);
pub(crate) const HEADER_LEN: u64 = 32;

pub(crate) struct Header {
    pub(crate) entry_count: u64,
    pub(crate) off_seeds: u64,
    pub(crate) checksum: u64,
    pub(crate) rice_k: u8,
}
