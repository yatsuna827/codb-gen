//! ポケモンコロシアム「とにかくバトル」seed特定用データベース(FullDB / LightDB)生成ツール。
//!
//! 生成ロジックは PokemonCoRNGLibrary(C#)の RentalTeamRank / GCSlot と一致させている。
//! - 色回避(採用PIDがトレーナーTSVに対して色違いなら再抽選)を含む
//! - チーム定義は PokemonCOSeedDataBaseAPI の BattleTeamUltimate に一致
//!   (旧C++実装のキュウコン性別比 M1F1 は誤りで、正しくは M1F3)
//!
//! FullDB : 全2^32状態を7バトル分のコードでキー化。ファイル {c5+c6*24}.bin に
//!          seedKey = c4 + c3*24 + c2*24^2 + c1*24^3 + c0*24^4 の昇順で seed(u32 LE) のみを格納。
//! LightDB: 1バトル適用後に到達可能な状態(像)のみを対象に、後続7バトルでキー化。
//!          ファイル {c1+c2*24}.bin に (seedKey, 生成後seed) (u32 LE ×2) を
//!          seedKey = c3 + c4*24 + c5*24^2 + c6*24^3 + c7*24^4 の昇順で格納。
//! いずれも既存の FullDBSearcher / LightDBSearcher と互換。

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

const A: u32 = 0x343FD;
const B: u32 = 0x269EC3;

/// n ステップ分のジャンプ定数 (An, Bn): s' = An*s + Bn
const fn jump(n: u32) -> (u32, u32) {
    let (mut ra, mut rb) = (1u32, 0u32);
    let (mut a, mut b) = (A, B);
    let mut n = n;
    while n > 0 {
        if n & 1 == 1 {
            ra = ra.wrapping_mul(a);
            rb = rb.wrapping_mul(a).wrapping_add(b);
        }
        b = b.wrapping_mul(a.wrapping_add(1));
        a = a.wrapping_mul(a);
        n >>= 1;
    }
    (ra, rb)
}

const J5: (u32, u32) = jump(5); // dummyPID(2) + IVs(2) + ability(1)

#[inline(always)]
fn rand(s: &mut u32) -> u32 {
    *s = s.wrapping_mul(A).wrapping_add(B);
    *s >> 16
}

/// 固定枠の生成条件。gender: 0=判定なし(性別不明), 1=♂, 2=♀
#[derive(Clone, Copy)]
struct Slot {
    ratio: u32,
    gender: u8,
    nature: u32,
}

const fn sl(ratio: u32, gender: u8, nature: u32) -> Slot {
    Slot { ratio, gender, nature }
}

const NG: u8 = 0;
const M: u8 = 1;
const F: u8 = 2;

// 性格: Hardy=0..Quirky=24 (C# Nature enum と同順)
// 性別比: M7F1=0x1F, M3F1=0x3F, M1F1=0x7F, M1F3=0xBF, 性別不明=ratio不問(gender=NG)
#[rustfmt::skip]
const TEAMS: [[Slot; 6]; 8] = [
    // 0: バシャーモ組
    [sl(0x1F, M, 22), sl(0x7F, F, 21), sl(0x7F, F, 15), sl(0x7F, M, 19), sl(0xBF, M, 4), sl(0x7F, F, 4)],
    // 1: エンテイ組
    [sl(0, NG, 11), sl(0x7F, F, 8), sl(0x7F, M, 1), sl(0x7F, M, 16), sl(0x7F, F, 16), sl(0x7F, M, 12)],
    // 2: ラグラージ組
    [sl(0x1F, M, 2), sl(0x3F, F, 16), sl(0x7F, M, 15), sl(0x7F, F, 18), sl(0x7F, M, 15), sl(0x7F, F, 3)],
    // 3: ライコウ組 (キュウコンは M1F3)
    [sl(0, NG, 16), sl(0xBF, F, 19), sl(0x7F, F, 3), sl(0x7F, F, 22), sl(0x1F, M, 3), sl(0x7F, M, 24)],
    // 4: メガニウム組
    [sl(0x1F, M, 17), sl(0x1F, M, 16), sl(0x1F, M, 15), sl(0x1F, M, 19), sl(0x1F, M, 5), sl(0x7F, F, 4)],
    // 5: スイクン組
    [sl(0, NG, 15), sl(0x7F, F, 17), sl(0, NG, 1), sl(0x7F, M, 3), sl(0, NG, 19), sl(0x7F, F, 3)],
    // 6: メタグロス組
    [sl(0, NG, 1), sl(0x1F, M, 8), sl(0x3F, M, 3), sl(0x7F, F, 1), sl(0x7F, F, 3), sl(0x3F, M, 3)],
    // 7: ヘラクロス組
    [sl(0x7F, F, 3), sl(0x7F, M, 10), sl(0x7F, F, 15), sl(0x7F, M, 3), sl(0x7F, F, 15), sl(0x7F, M, 3)],
];

/// GCSlot.Use 相当: dummyPID(2) + IVs(2) + ability(1) + PID再抽選ループ(色回避込み)
///
/// 判定はブランチレスにまとめ、試行ごとの分岐を1つ(継続/採用)に抑える。
/// 性別判定の結果はほぼ乱数なので、分岐にすると毎試行ミスプレディクトが発生する。
#[inline(always)]
fn gen_slot(s: &mut u32, slot: &Slot, tsv: u32) {
    *s = s.wrapping_mul(J5.0).wrapping_add(J5.1);
    let check_gender = slot.gender != NG;
    let want_female = slot.gender == F;
    loop {
        let hi = rand(s);
        let lo = rand(s);
        let pid = (hi << 16) | lo;
        let g_ok = !check_gender | (((lo & 0xFF) < slot.ratio) == want_female);
        let n_ok = pid % 25 == slot.nature;
        let s_ok = (hi ^ lo ^ tsv) >= 8; // 色回避
        if g_ok & n_ok & s_ok {
            return;
        }
    }
}

/// RentalTeamRank.GenerateCode 相当。code = 名前*8 + 自チーム ∈ [0,24)
#[inline(always)]
fn battle(s: &mut u32) -> u32 {
    let e = (rand(s) & 7) as usize;
    let p = loop {
        let p = (rand(s) & 7) as usize;
        if p != e {
            break p;
        }
    };
    let etsv = rand(s) ^ rand(s);
    for slot in &TEAMS[e] {
        gen_slot(s, slot, etsv);
    }
    let name = rand(s) % 3;
    let ptsv = rand(s) ^ rand(s);
    for slot in &TEAMS[p] {
        gen_slot(s, slot, ptsv);
    }
    name * 8 + p as u32
}

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
                        c[0] = battle(&mut s);
                        let v = s; // 1バトル適用後の状態 (像)

                        // 像を最初に確保できた場合のみ LightDB のキー計算が必要になる。
                        // 確保できるのは全状態の約3%なので、後続バトルの計算は
                        // 必要になった時点まで遅延する。
                        let claimed = do_light && {
                            let idx = (v >> 6) as usize;
                            let bit = 1u64 << (v & 63);
                            bitmap[idx].fetch_or(bit, Ordering::Relaxed) & bit == 0
                        };

                        if let Some(sink) = &full_sink {
                            for k in 1..7 {
                                c[k] = battle(&mut s);
                            }
                            let file = (c[5] + c[6] * 24) as usize;
                            let key = c[4] + c[3] * 24 + c[2] * 576 + c[1] * 13824 + c[0] * 331776;
                            fbufs.push(file, (key, s0), sink);
                            if claimed {
                                c[7] = battle(&mut s);
                            }
                        } else if claimed {
                            for k in 1..8 {
                                c[k] = battle(&mut s);
                            }
                        }

                        if claimed {
                            let sink = light_sink.as_ref().unwrap();
                            let file = (c[1] + c[2] * 24) as usize;
                            let key = c[3] + c[4] * 24 + c[5] * 576 + c[6] * 13824 + c[7] * 331776;
                            lbufs.push(file, (key, s), sink); // s = 7バトル生成後の状態
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

/// C#ライブラリとの突き合わせ用: 1バトル分の (seed, code, 生成後seed) を出力する
fn selftest(n: u32) {
    for i in 0..n {
        let seed = i.wrapping_mul(0x9E3779B9);
        let mut s = seed;
        let code = battle(&mut s);
        println!("{:08X} {:02} {:08X}", seed, code, s);
    }
}

/// 生成済みDBの簡易検証: キーの単調性とエントリ数を確認する
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
  codb-gen selftest [count]
  codb-gen verify <DIR> [--pairs]";

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
        _ => eprintln!("{}", usage),
    }
}
