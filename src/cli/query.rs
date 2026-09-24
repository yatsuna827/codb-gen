use std::fs::File;
use std::path::Path;

use crate::core::teamgen::generate_team_checked;
use crate::format::reader::{read_at, read_header, CountsReader};
use crate::format::{Header, HEADER_LEN};

/// 観測コード列c1..c7で検索し、(起点seed, 7回生成後seed)を返す。
pub fn query(path: &Path, codes: &[u32; 7]) -> Vec<(u32, u32)> {
    let mut f = File::open(path).expect("open db");
    let h = read_header(&mut f);

    for &c in codes {
        assert!(c < 24, "code out of range");
    }
    let mut p = 0u64;
    for &c in &codes[0..5] {
        p = p * 24 + c as u64;
    }

    // 個数列をロードし、先頭からPまで復号して区間[lo, hi)を得る。
    let table_len = (h.off_seeds - HEADER_LEN) as usize;
    let mut counts_buf = vec![0u8; table_len];
    read_at(&mut f, HEADER_LEN, &mut counts_buf);

    let mut reader = CountsReader::new(&counts_buf, h.rice_k);
    let mut lo = 0u64;
    for _ in 0..p {
        lo += reader.read() as u64;
    }
    let hi = lo + reader.read() as u64;

    query_range(&mut f, &h, codes, lo, hi)
}

fn query_range(f: &mut File, h: &Header, codes: &[u32; 7], lo: u64, hi: u64) -> Vec<(u32, u32)> {
    let mut results = Vec::new();
    if lo >= hi {
        return results;
    }
    let n = (hi - lo) as usize;
    let mut seedbuf = vec![0u8; n * 2];
    read_at(f, h.off_seeds + lo * 2, &mut seedbuf);

    for sb in seedbuf.chunks_exact(2) {
        let hi16 = u16::from_le_bytes(sb.try_into().unwrap()) as u32;
        for x in 0..0x10000u32 {
            let v = (hi16 << 16) | x;
            let mut s = v;
            let mut ok = true;
            for &c in codes {
                if !generate_team_checked(&mut s, c) {
                    ok = false;
                    break;
                }
            }
            if ok {
                results.push((v, s));
            }
        }
    }
    results
}
