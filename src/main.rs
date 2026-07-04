//! ポケモンコロシアム「とにかくバトル」seed特定用データベース生成ツール。
//!
//! チーム生成ロジックは src/teamgen.rs(PokemonCoRNGLibrary と一致)。
//! 観測1回 = 組み合わせ画面のチーム生成1回分であり、コードは
//! COMトレーナー名(3) × 自チーム(8) = 24通り。
//!
//! FullDB : 全2^32状態を7回のチーム生成コードでキー化。ファイル {c5+c6*24}.bin に
//!          seedKey = c4 + c3*24 + c2*24^2 + c1*24^3 + c0*24^4 の昇順で seed(u32 LE) のみを格納。
//! LightDB: 1回のチーム生成適用後に到達可能な状態(像)のみを対象に、後続7回でキー化。
//!          ファイル {c1+c2*24}.bin に (seedKey, 生成後seed) (u32 LE ×2) を
//!          seedKey = c3 + c4*24 + c5*24^2 + c6*24^3 + c7*24^4 の昇順で格納。
//! いずれも既存の FullDBSearcher / LightDBSearcher と互換。
//!
//! 圧縮LightDB(単一ファイル、gen-light / query / verify-light): src/clight.rs 参照。

mod clight;
mod teamgen;

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use teamgen::generate_team;

// ---------------------------------------------------------------------------

const FILES: usize = 576;
const BUF_CAP: usize = 1024;
const CHUNK: u64 = 1 << 20;

struct Sink {
    files: Vec<Mutex<BufWriter<File>>>,
    count: AtomicU64,
}

impl Sink {
    fn new(dir: &Path, prefix: &str) -> Sink {
        fs::create_dir_all(dir).expect("create dir");
        let files = (0..FILES)
            .map(|i| {
                let f = File::create(dir.join(format!("{}{}.bin", prefix, i))).expect("create file");
                Mutex::new(BufWriter::with_capacity(1 << 16, f))
            })
            .collect();
        Sink { files, count: AtomicU64::new(0) }
    }

    fn flush_buf(&self, idx: usize, buf: &mut Vec<(u32, u32)>) {
        if buf.is_empty() {
            return;
        }
        let mut bytes = Vec::with_capacity(buf.len() * 8);
        for &(k, v) in buf.iter() {
            bytes.extend_from_slice(&k.to_le_bytes());
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        self.count.fetch_add(buf.len() as u64, Ordering::Relaxed);
        buf.clear();
        let mut w = self.files[idx].lock().unwrap();
        w.write_all(&bytes).expect("write");
    }

    fn finish(self) -> u64 {
        for m in self.files {
            m.into_inner().unwrap().flush().expect("flush");
        }
        self.count.load(Ordering::Relaxed)
    }
}

struct ThreadBufs(Vec<Vec<(u32, u32)>>);

impl ThreadBufs {
    fn new() -> ThreadBufs {
        ThreadBufs((0..FILES).map(|_| Vec::with_capacity(BUF_CAP)).collect())
    }

    #[inline(always)]
    fn push(&mut self, idx: usize, entry: (u32, u32), sink: &Sink) {
        let buf = &mut self.0[idx];
        buf.push(entry);
        if buf.len() == BUF_CAP {
            sink.flush_buf(idx, &mut self.0[idx]);
        }
    }

    fn flush_all(&mut self, sink: &Sink) {
        for i in 0..FILES {
            sink.flush_buf(i, &mut self.0[i]);
        }
    }
}

fn generate(out: &Path, do_full: bool, do_light: bool, limit: u64, threads: usize) {
    let start = Instant::now();
    let full_dir = out.join("FullDB");
    let light_dir = out.join("LightDB");

    let full_sink = if do_full { Some(Sink::new(&full_dir, "tmp_")) } else { None };
    let light_sink = if do_light { Some(Sink::new(&light_dir, "tmp_")) } else { None };

    // 像の重複排除用ビットマップ (512 MiB)
    let bitmap: Vec<AtomicU64> = if do_light {
        (0..(1usize << 26)).map(|_| AtomicU64::new(0)).collect()
    } else {
        Vec::new()
    };

    let counter = AtomicU64::new(0);
    let done = AtomicU64::new(0);

    eprintln!(
        "generate: states=0x{:X}, threads={}, full={}, light={}",
        limit, threads, do_full, do_light
    );

    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| {
                let mut fbufs = ThreadBufs::new();
                let mut lbufs = ThreadBufs::new();
                loop {
                    let lo = counter.fetch_add(CHUNK, Ordering::Relaxed);
                    if lo >= limit {
                        break;
                    }
                    let hi = (lo + CHUNK).min(limit);
                    let mut c = [0u32; 8];
                    for s0 in lo..hi {
                        let s0 = s0 as u32;
                        let mut s = s0;
                        c[0] = generate_team(&mut s);
                        let v = s; // 1回のチーム生成適用後の状態 (像)

                        // 像を最初に確保できた場合のみ LightDB のキー計算が必要になる。
                        // 確保できるのは全状態の約3%なので、後続のチーム生成の計算は
                        // 必要になった時点まで遅延する。
                        let claimed = do_light && {
                            let idx = (v >> 6) as usize;
                            let bit = 1u64 << (v & 63);
                            bitmap[idx].fetch_or(bit, Ordering::Relaxed) & bit == 0
                        };

                        if let Some(sink) = &full_sink {
                            for k in 1..7 {
                                c[k] = generate_team(&mut s);
                            }
                            let file = (c[5] + c[6] * 24) as usize;
                            let key = c[4] + c[3] * 24 + c[2] * 576 + c[1] * 13824 + c[0] * 331776;
                            fbufs.push(file, (key, s0), sink);
                            if claimed {
                                c[7] = generate_team(&mut s);
                            }
                        } else if claimed {
                            for k in 1..8 {
                                c[k] = generate_team(&mut s);
                            }
                        }

                        if claimed {
                            let sink = light_sink.as_ref().unwrap();
                            let file = (c[1] + c[2] * 24) as usize;
                            let key = c[3] + c[4] * 24 + c[5] * 576 + c[6] * 13824 + c[7] * 331776;
                            lbufs.push(file, (key, s), sink); // s = 7回生成後の状態
                        }
                    }
                    let d = done.fetch_add(hi - lo, Ordering::Relaxed) + (hi - lo);
                    if (d / CHUNK) % 64 == 0 {
                        let el = start.elapsed().as_secs_f64();
                        let rate = d as f64 / el;
                        eprintln!(
                            "  {:>5.1}% {:>7.1}s  {:.1}M states/s  ETA {:.0}s",
                            d as f64 / limit as f64 * 100.0,
                            el,
                            rate / 1e6,
                            (limit - d) as f64 / rate
                        );
                    }
                }
                if let Some(sink) = &full_sink {
                    fbufs.flush_all(sink);
                }
                if let Some(sink) = &light_sink {
                    lbufs.flush_all(sink);
                }
            });
        }
    });

    let full_n = full_sink.map(|s| s.finish());
    let light_n = light_sink.map(|s| s.finish());
    eprintln!("simulation done in {:.1}s", start.elapsed().as_secs_f64());
    if let Some(n) = full_n {
        eprintln!("FullDB entries : {}", n);
    }
    if let Some(n) = light_n {
        eprintln!(
            "LightDB entries: {} ({:.4}% of 2^32)",
            n,
            n as f64 / 4294967296.0 * 100.0
        );
    }

    // ソートフェーズ
    if do_full {
        sort_phase(&full_dir, false, threads);
    }
    if do_light {
        sort_phase(&light_dir, true, threads);
    }
    eprintln!("all done in {:.1}s", start.elapsed().as_secs_f64());
}

/// tmp_{i}.bin (key,value ペア) を読み、key 昇順にソートして {i}.bin へ書き出す。
/// keep_pairs=false なら value(seed) のみを書く(FullDB形式)。
fn sort_phase(dir: &Path, keep_pairs: bool, threads: usize) {
    let start = Instant::now();
    let next = AtomicUsize::new(0);
    std::thread::scope(|sc| {
        for _ in 0..threads.min(8) {
            sc.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= FILES {
                    break;
                }
                let tmp = dir.join(format!("tmp_{}.bin", i));
                let bytes = fs::read(&tmp).expect("read tmp");
                let mut entries: Vec<(u32, u32)> = bytes
                    .chunks_exact(8)
                    .map(|c| {
                        (
                            u32::from_le_bytes([c[0], c[1], c[2], c[3]]),
                            u32::from_le_bytes([c[4], c[5], c[6], c[7]]),
                        )
                    })
                    .collect();
                drop(bytes);
                entries.sort_unstable();
                let mut out = Vec::with_capacity(entries.len() * if keep_pairs { 8 } else { 4 });
                for &(k, v) in &entries {
                    if keep_pairs {
                        out.extend_from_slice(&k.to_le_bytes());
                    }
                    out.extend_from_slice(&v.to_le_bytes());
                }
                fs::write(dir.join(format!("{}.bin", i)), &out).expect("write final");
                fs::remove_file(&tmp).expect("remove tmp");
            });
        }
    });
    eprintln!(
        "sort {} done in {:.1}s",
        dir.display(),
        start.elapsed().as_secs_f64()
    );
}

/// C#ライブラリとの突き合わせ用: 1回のチーム生成の (seed, code, 生成後seed) を出力する
fn selftest(n: u32) {
    for i in 0..n {
        let seed = i.wrapping_mul(0x9E3779B9);
        let mut s = seed;
        let code = generate_team(&mut s);
        println!("{:08X} {:02} {:08X}", seed, code, s);
    }
}

/// 生成済みDB(576ファイル形式)の簡易検証: キーの単調性とエントリ数を確認する
fn verify(dir: &Path, pairs: bool) {
    let mut total = 0u64;
    for i in 0..FILES {
        let path = dir.join(format!("{}.bin", i));
        let bytes = fs::read(&path).expect("read");
        let unit = if pairs { 8 } else { 4 };
        assert!(bytes.len() % unit == 0, "size not aligned: {}", path.display());
        total += (bytes.len() / unit) as u64;
        if pairs {
            let mut prev = 0u32;
            for c in bytes.chunks_exact(8) {
                let k = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                assert!(k >= prev, "not sorted: {} at key {}", path.display(), k);
                prev = k;
            }
        }
    }
    println!("{}: {} entries, OK", dir.display(), total);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let usage = "usage:
  codb-gen gen --out <DIR> [--full-only|--light-only] [--limit <N>] [--threads <N>]
  codb-gen gen-light --out <FILE> [--limit <N>] [--threads <N>]
  codb-gen query <FILE> (<c1..c7> | <c0..c7> | --from-seed <HEXSEED>)
  codb-gen selftest [count]
  codb-gen verify <DIR> [--pairs]
  codb-gen verify-light <FILE>
  codb-gen convert <IN> <OUT> <cuml|cnts>";

    match args.get(1).map(|s| s.as_str()) {
        Some("selftest") => {
            let n = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(64);
            selftest(n);
        }
        Some("verify") => {
            let dir = args.get(2).expect(usage);
            let pairs = args.iter().any(|a| a == "--pairs");
            verify(Path::new(dir), pairs);
        }
        Some("verify-light") => {
            let file = args.get(2).expect(usage);
            clight::verify(Path::new(file));
        }
        Some("convert") => {
            let input = args.get(2).expect(usage);
            let output = args.get(3).expect(usage);
            let target = args.get(4).expect(usage);
            clight::convert(Path::new(input), Path::new(output), target);
        }
        Some("query") => {
            let file = args.get(2).expect(usage);
            let rest: Vec<&String> = args.iter().skip(3).collect();
            let codes: [u32; 7] = if rest.len() == 2 && rest[0] == "--from-seed" {
                // 既知seedから観測8回分を生成して検索する自己テスト。
                // c0(先頭観測)は捨て、期待値(8回生成後seed)との照合まで行う。
                let s0 = u32::from_str_radix(rest[1], 16).expect("hex seed");
                let mut s = s0;
                let mut cs = [0u32; 8];
                for c in cs.iter_mut() {
                    *c = generate_team(&mut s);
                }
                println!(
                    "seed={:08X} codes={:?} expect={:08X}",
                    s0,
                    cs,
                    s
                );
                let codes: [u32; 7] = cs[1..8].try_into().unwrap();
                let results = clight::query(Path::new(file), &codes);
                let hit = results.iter().any(|&(_, fin)| fin == s);
                for (v, fin) in &results {
                    println!("origin={:08X} result={:08X}", v, fin);
                }
                println!("{}", if hit { "HIT" } else { "MISS" });
                std::process::exit(if hit { 0 } else { 1 });
            } else {
                // コード列指定: 7個なら c1..c7、8個なら先頭を捨てる
                let mut cs: Vec<u32> = rest.iter().map(|s| s.parse().expect("code")).collect();
                if cs.len() == 8 {
                    cs.remove(0);
                }
                cs.as_slice().try_into().expect("need 7 or 8 codes")
            };
            let results = clight::query(Path::new(file), &codes);
            for (v, fin) in &results {
                println!("origin={:08X} result={:08X}", v, fin);
            }
            eprintln!("{} match(es)", results.len());
        }
        Some("gen") => {
            let mut out: Option<PathBuf> = None;
            let mut do_full = true;
            let mut do_light = true;
            let mut limit: u64 = 1 << 32;
            let mut threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
            let mut it = args.iter().skip(2);
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--out" => out = Some(PathBuf::from(it.next().expect("--out DIR"))),
                    "--full-only" => do_light = false,
                    "--light-only" => do_full = false,
                    "--limit" => limit = it.next().expect("--limit N").parse().expect("limit"),
                    "--threads" => threads = it.next().expect("--threads N").parse().expect("threads"),
                    _ => panic!("unknown arg: {}\n{}", a, usage),
                }
            }
            let out = out.expect(usage);
            generate(&out, do_full, do_light, limit, threads);
        }
        Some("gen-light") => {
            let mut out: Option<PathBuf> = None;
            let mut limit: u64 = 1 << 32;
            let mut threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
            let mut it = args.iter().skip(2);
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--out" => out = Some(PathBuf::from(it.next().expect("--out FILE"))),
                    "--limit" => limit = it.next().expect("--limit N").parse().expect("limit"),
                    "--threads" => threads = it.next().expect("--threads N").parse().expect("threads"),
                    _ => panic!("unknown arg: {}\n{}", a, usage),
                }
            }
            let out = out.expect(usage);
            clight::generate(&out, limit, threads);
        }
        _ => eprintln!("{}", usage),
    }
}
