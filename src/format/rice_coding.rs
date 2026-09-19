// NOTE: ここでは、unary符号は0を終端記号とする

pub(super) struct RiceCodingWriter {
    buf: Vec<u8>,
    acc: u64,
    nbits: u32,
}
impl RiceCodingWriter {
    pub(super) fn new() -> Self {
        Self {
            buf: Vec::new(),
            acc: 0,
            nbits: 0,
        }
    }

    #[inline(always)]
    pub(super) fn write(&mut self, value: u32, k: u8) {
        self.write_unary(value >> k);
        if k > 0 {
            self.write_bits(value & ((1u32 << k) - 1), k as u32);
        }
    }

    #[inline]
    fn write_unary(&mut self, mut q: u32) {
        while q >= 24 {
            self.write_bits(0x00FF_FFFF, 24);
            q -= 24;
        }
        self.write_bits((1u32 << q) - 1, q + 1);
    }

    #[inline]
    fn write_bits(&mut self, value: u32, n: u32) {
        self.acc |= (value as u64) << self.nbits;
        self.nbits += n;
        while self.nbits >= 8 {
            self.buf.push((self.acc & 0xFF) as u8);
            self.acc >>= 8;
            self.nbits -= 8;
        }
    }

    pub(super) fn build(mut self) -> Vec<u8> {
        if self.nbits > 0 {
            self.buf.push((self.acc & 0xFF) as u8);
        }
        while self.buf.len() % 8 != 0 {
            self.buf.push(0);
        }
        self.buf
    }
}

pub(super) struct RiceCodingReader<'a> {
    buf: &'a [u8],
    byte_idx: usize,
    bit_idx: u32,
}
impl<'a> RiceCodingReader<'a> {
    pub(super) fn new(buf: &'a [u8]) -> Self {
        Self {
            buf,
            byte_idx: 0,
            bit_idx: 0,
        }
    }

    #[inline(always)]
    pub(super) fn read(&mut self, k: u8) -> u32 {
        let q = self.read_unary();
        let r = if k > 0 { self.read_bits(k as u32) } else { 0 };
        (q << k) | r
    }

    #[inline(always)]
    fn read_unary(&mut self) -> u32 {
        let mut q = 0u32;
        while self.read_bit() == 1 {
            q += 1;
        }
        q
    }

    #[inline(always)]
    fn read_bits(&mut self, n: u32) -> u32 {
        let mut v = 0u32;
        for i in 0..n {
            v |= self.read_bit() << i;
        }
        v
    }

    #[inline(always)]
    fn read_bit(&mut self) -> u32 {
        let b = (self.buf[self.byte_idx] >> self.bit_idx) & 1;
        self.bit_idx += 1;
        if self.bit_idx == 8 {
            self.bit_idx = 0;
            self.byte_idx += 1;
        }
        b as u32
    }
}
