use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

mod orbit;
mod teamgen_optimized;

use crate::format::{
    choose_optimal_k, encode_counts, fnv1a64, FNV_INIT, HEADER_LEN, MAGIC, P_COUNT, VERSION,
};
use orbit::scan_arc;

/// (code上位, code下位, 7回生成後seed, 起点seed) \
/// 辞書式順序でソートすることで、code → 7回生成後seed → 起点seed でのソートができる。
type Entry = [u32; 4];

pub fn generate(out: &Path, limit: Option<u64>, threads: usize) {
    let start = Instant::now();
    if let Some(limit) = limit {
        eprintln!("gen: limit=0x{:X}, threads={}", limit, threads);
    } else {
        eprintln!("gen: threads={}", threads);
    }
    let limit = limit.unwrap_or(1 << 32);

    // Phase 1: エントリを全列挙する
    let t1 = Instant::now();
    let mut buckets: Vec<Vec<Entry>> = (0..24).map(|_| Vec::new()).collect();
    let threads_u = threads.max(1) as u64;
    {
        let base = limit / threads_u;
        let rem = limit % threads_u;
        let results = std::sync::Mutex::new(&mut buckets);
        std::thread::scope(|sc| {
            let mut arc_start = 0u64;
            for t in 0..threads_u {
                let len = base + if t < rem { 1 } else { 0 };
                let arc_end = arc_start + len;
                let (a0, a1) = (arc_start as i64, arc_end as i64);

                let results = &results;
                sc.spawn(move || {
                    let local = scan_arc(a0, a1);
                    let mut buckets = results.lock().unwrap();
                    for (b, mut l) in buckets.iter_mut().zip(local.into_iter()) {
                        b.append(&mut l);
                    }
                });

                arc_start = arc_end;
            }
        });
    }

    let total: usize = buckets.iter().map(|b| b.len()).sum();
    eprintln!(
        "phase1 (orbit scan) done in {:.1}s, entries={}",
        t1.elapsed().as_secs_f64(),
        total,
    );

    // Phase 2: バケットごとにソートと、code+生成後seedでの重複除去を行う
    let t2 = Instant::now();
    let dedup_count = AtomicUsize::new(0);
    {
        let next = AtomicUsize::new(0);
        let slots: Vec<std::sync::Mutex<&mut Vec<Entry>>> =
            buckets.iter_mut().map(std::sync::Mutex::new).collect();
        std::thread::scope(|sc| {
            for _ in 0..threads.min(24) {
                sc.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= 24 {
                        break;
                    }
                    let mut b = slots[i].lock().unwrap();
                    b.sort_unstable();
                    b.dedup_by(|a, c| a[0] == c[0] && a[1] == c[1] && a[2] == c[2]);
                    dedup_count.fetch_add(b.len(), Ordering::Relaxed);
                });
            }
        });
    }
    let raw_total = total;
    let total: usize = dedup_count.load(Ordering::Relaxed);
    eprintln!(
        "phase2 (sort+dedup) done in {:.1}s, entries={} (raw {})",
        t2.elapsed().as_secs_f64(),
        total,
        raw_total
    );

    // Phase 3: 書き出し
    let t3 = Instant::now();
    let mut counts = vec![0u32; P_COUNT];
    let mut seeds: Vec<u8> = Vec::with_capacity(total * 2);
    for b in &buckets {
        for e in b {
            let k = ((e[0] as u64) << 32) | e[1] as u64;
            counts[(k / 576) as usize] += 1;
            seeds.extend_from_slice(&((e[3] >> 16) as u16).to_le_bytes());
        }
    }
    drop(buckets);

    let sum: u64 = counts.iter().map(|&c| c as u64).sum();
    assert!(sum == total as u64, "counts total != entry count");
    let rice_k = choose_optimal_k(&counts);
    let counts_bytes = encode_counts(&counts, rice_k);
    drop(counts);

    let mut checksum = FNV_INIT;
    fnv1a64(&mut checksum, &counts_bytes);
    fnv1a64(&mut checksum, &seeds);

    let off_seeds = HEADER_LEN + counts_bytes.len() as u64;

    let mut header = [0u8; HEADER_LEN as usize];
    header[0..4].copy_from_slice(&MAGIC);
    header[4..8].copy_from_slice(&VERSION.to_le_bytes());
    header[8..16].copy_from_slice(&off_seeds.to_le_bytes());
    header[16..24].copy_from_slice(&checksum.to_le_bytes());
    header[24] = rice_k;

    eprintln!(
        "counts section: k={}, {} bytes (entries={})",
        rice_k,
        counts_bytes.len(),
        total
    );

    let mut w = BufWriter::with_capacity(1 << 20, File::create(out).expect("create out"));
    w.write_all(&header).expect("write");
    w.write_all(&counts_bytes).expect("write");
    w.write_all(&seeds).expect("write");
    w.flush().expect("flush");
    eprintln!("phase3 (write) done in {:.1}s", t3.elapsed().as_secs_f64());
    eprintln!(
        "gen all done in {:.1}s, {} entries, {} bytes",
        start.elapsed().as_secs_f64(),
        total,
        off_seeds + total as u64 * 2
    );
}
