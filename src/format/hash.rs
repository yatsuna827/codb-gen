pub(crate) fn fnv1a64(h: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *h ^= b as u64;
        *h = h.wrapping_mul(0x100_0000_01b3);
    }
}

pub(crate) const FNV_INIT: u64 = 0xCBF2_9CE4_8422_2325;
