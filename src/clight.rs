//! LightDB 圧縮フォーマット(単一ファイル)。仕様は docs/design-compressed-lightdb.md 参照。
//! めんどくせえ、外部ドキュメントに書いてあるならここに書く必要ねえだろ
//!
//! キー: 観測1〜7回目のチーム生成コード c1..c7(各 [0,24))に対し
//!   K = c1*(24^6) + c2*(24^5) + ... + c7  (< 24^7、33bit)
//! また、前の5回分だけを取り出した値Pを Prefix と呼ぶ（24進数と見れば、前から5文字であるため）
//!   P = c1*(24^4) + c2*(24^3) + c3*(24^2) + c4*24 + c5
//!
//! 圧縮LightDBは CUML(累積和形式)**と**CNTS(個数形式)**の2種類ある。
//!   - CUML: Prefix P(24^5通り)ごとのエントリ数の累積和(u32)をそのまま並べたテーブル。
//!     オープン時のロードが不要で、検索は表を2箇所seekするだけで完結する(表は31.85MB)。
//!   - CNTS: Prefix Pごとのエントリ数を6bitでビットパックした個数列。ファイルサイズが
//!     最小になる代わりに、オープン時に個数列をロードし検索のたびに先頭からの線形和
//!     (累積和相当)を構築する必要がある(表は5.97MB)。
//!
//! ファイルレイアウト(すべてリトルエンディアン、両形式共通):
//!   offset 0  : ヘッダ 64 B
//!     +0  magic    [u8; 8] = "COLIGHT\0" (ファミリー識別、両形式で不変)
//!     +8  format   [u8; 4] = "CUML" または "CNTS" (FourCC、ASCII)
//!     +12 version  u32 = 1 (形式ごとに1から振り直す)
//!     +16 entry_count u64
//!     +24 表セクションオフセット u64 (= 64。CUMLなら累積和表、CNTSなら個数列)
//!     +32 部分seed配列オフセット u64
//!         (CUML: 64 + 24^5*4   = 31,850,560)
//!         (CNTS: 64 + 24^5*6/8 =  5,972,032)
//!     +40 checksum u64 (表セクション+部分seed配列のFNV-1a 64、この順)
//!     +48〜63 (予約) = 0
//!   表セクション:
//!     CUML: Prefix P(24^5通り)ごとのエントリ数の累積和を先頭からu32(LE)で並べたテーブル。
//!           要素iの値は「P<=iであるエントリ数の総和」。
//!     CNTS: Prefix Pごとのエントリ数を、先頭から6bitずつLSBファーストでビットパックした列。
//!           ちょうど5,971,968バイト(24^5*6bit)。個数の上限63はフォーマット制約
//!           (実測最大43)であり、生成・変換時にassertで保証する。
//!   部分seed配列：エントリのK昇順に、観測1回目開始時点のseedの上位16bitを並べたテーブル
//!     (両形式共通)。
//!
//! キーはエントリのソートに使う。c1 ~ c5から計算されるPrefixは表セクションに反映されるが
//! (値そのものはファイルに保存されない)、c6・c7はソートにのみ使われる。
//! 検索は、CUMLなら表を2箇所seekして区間 [lo, hi) を直接得る。CNTSなら個数列の先頭からPまでの
//! 和で区間開始を求め(線形和、約800万回の6bit展開で数ms)、区間終了は開始+個数[P]で得る。
//! 区間内(平均16.1エントリ)の全エントリについて部分seedの下位16bitを全探索し、7回分のコードを
//! 入力値と照合(`generate_team_checked`)して、完全一致するものだけを返す。
//!

use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use crate::teamgen::{generate_team, generate_team_checked};

pub const MAGIC: [u8; 8] = *b"COLIGHT\0";
const FORMAT_CUML: [u8; 4] = *b"CUML";
const FORMAT_CNTS: [u8; 4] = *b"CNTS";
const NEW_VERSION: u32 = 1;
const P_COUNT: usize = 7_962_624; // 24^5
const HEADER_LEN: u64 = 64;
const COUNTS_BYTES: u64 = (P_COUNT as u64 * 6) / 8; // 5,971,968
const CUML_BYTES: u64 = P_COUNT as u64 * 4; // 31,850,496

fn fnv1a64(h: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *h ^= b as u64;
        *h = h.wrapping_mul(0x100_0000_01b3);
    }
}
const FNV_INIT: u64 = 0xcbf2_9ce4_8422_2325;

/// 個数(各値63以下)の列を、先頭から6bitずつLSBファーストでビットパックする。
fn pack6(counts: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity((counts.len() * 6 + 7) / 8);
    let mut acc: u64 = 0;
    let mut nbits: u32 = 0;
    for &c in counts {
        assert!(c <= 63, "count exceeds 6-bit limit (max 63): {}", c);
        acc |= (c as u64) << nbits;
        nbits += 6;
        while nbits >= 8 {
            out.push((acc & 0xFF) as u8);
            acc >>= 8;
            nbits -= 8;
        }
    }
    assert!(nbits == 0, "counts length does not align to a byte boundary");
    out
}

/// 6bitビットパックされた個数列を先頭から`count`個展開する。
fn unpack6(buf: &[u8], count: usize) -> Vec<u32> {
    let mut out = Vec::with_capacity(count);
    let mut acc: u64 = 0;
    let mut nbits: u32 = 0;
    let mut byte_idx = 0usize;
    for _ in 0..count {
        while nbits < 6 {
            acc |= (buf[byte_idx] as u64) << nbits;
            byte_idx += 1;
            nbits += 8;
        }
        out.push((acc & 0x3F) as u32);
        acc >>= 6;
        nbits -= 6;
    }
    out
}

/// 6bitビットパックされた個数列を先頭から`p`番目まで走査し、
/// (個数[0..p]の総和, 個数[p]) を返す。
fn scan_prefix(counts_buf: &[u8], p: usize) -> (u64, u32) {
    let mut acc: u64 = 0;
    let mut nbits: u32 = 0;
    let mut byte_idx = 0usize;
    let mut sum = 0u64;
    let mut val = 0u32;
    for i in 0..=p {
        while nbits < 6 {
            acc |= (counts_buf[byte_idx] as u64) << nbits;
            byte_idx += 1;
            nbits += 8;
        }
        let v = (acc & 0x3F) as u32;
        acc >>= 6;
        nbits -= 6;
        if i == p {
            val = v;
        } else {
            sum += v as u64;
        }
    }
    (sum, val)
}

/// (K, 7回生成後seed, 起点seed)。K は 33 bit なので上位を e[0](0/1)、下位を e[1] に分ける。
/// e[2] = 7回生成後seed、e[3] = 起点seed。[u32; 4] の辞書式順序でソートすることで
/// K → 7回生成後seed → 起点seed の順に整列される。
type Entry = [u32; 4];

// ---------------------------------------------------------------------------
// 生成

pub fn generate(out: &Path, limit: u64, threads: usize) {
    let start = Instant::now();
    eprintln!("gen-light: states=0x{:X}, threads={}", limit, threads);

    // Phase 1: 全状態に1回チーム生成を適用し、像をビットマップで重複排除
    let bitmap: Vec<AtomicU64> = (0..(1usize << 26)).map(|_| AtomicU64::new(0)).collect();
    {
        const CHUNK: u64 = 1 << 20;
        let counter = AtomicU64::new(0);
        std::thread::scope(|sc| {
            for _ in 0..threads {
                sc.spawn(|| loop {
                    let lo = counter.fetch_add(CHUNK, Ordering::Relaxed);
                    if lo >= limit {
                        break;
                    }
                    let hi = (lo + CHUNK).min(limit);
                    for s0 in lo..hi {
                        let mut s = s0 as u32;
                        generate_team(&mut s);
                        bitmap[(s >> 6) as usize].fetch_or(1u64 << (s & 63), Ordering::Relaxed);
                    }
                });
            }
        });
    }
    eprintln!("phase1 (image bitmap) done in {:.1}s", start.elapsed().as_secs_f64());

    // Phase 2: 各像からコード c1..c7 を生成してキー化。c1 (= K / 24^6) で24バケツに分ける
    let t2 = Instant::now();
    let mut buckets: Vec<Vec<Entry>> = (0..24).map(|_| Vec::new()).collect();
    {
        const WORDS_CHUNK: usize = 1 << 14;
        let counter = AtomicUsize::new(0);
        let results = std::sync::Mutex::new(&mut buckets);
        std::thread::scope(|sc| {
            for _ in 0..threads {
                sc.spawn(|| {
                    let mut local: Vec<Vec<Entry>> = (0..24).map(|_| Vec::new()).collect();
                    loop {
                        let wlo = counter.fetch_add(WORDS_CHUNK, Ordering::Relaxed);
                        if wlo >= bitmap.len() {
                            break;
                        }
                        let whi = (wlo + WORDS_CHUNK).min(bitmap.len());
                        for w in wlo..whi {
                            let mut bits = bitmap[w].load(Ordering::Relaxed);
                            while bits != 0 {
                                let v = ((w as u32) << 6) | bits.trailing_zeros();
                                bits &= bits - 1;
                                let mut s = v;
                                let c1 = generate_team(&mut s);
                                let mut k = c1 as u64;
                                for _ in 0..6 {
                                    k = k * 24 + generate_team(&mut s) as u64;
                                }
                                local[c1 as usize].push([(k >> 32) as u32, k as u32, s, v]);
                            }
                        }
                    }
                    let mut buckets = results.lock().unwrap();
                    for (b, mut l) in buckets.iter_mut().zip(local.drain(..)) {
                        b.append(&mut l);
                    }
                });
            }
        });
    }
    drop(bitmap);
    let total: usize = buckets.iter().map(|b| b.len()).sum();
    eprintln!(
        "phase2 (keying) done in {:.1}s, entries={}",
        t2.elapsed().as_secs_f64(),
        total
    );

    // Phase 3: バケツごとに [u32; 4] の辞書式順(K→7回生成後seed→起点seed)にソートし、
    // 先頭3要素(K, 7回生成後seed)が等しい連続エントリを1件に縮約する(起点seed最小の1件が残る)。
    // 合流重複は必ず同じKを持つのでバケツ内dedupのみで完全に除去できる。
    let t3 = Instant::now();
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
        "phase3 (sort+dedup) done in {:.1}s, entries={} (raw {})",
        t3.elapsed().as_secs_f64(),
        total,
        raw_total
    );

    // Phase 4: セクション構築 + 書き出し(CNTS形式)
    let t4 = Instant::now();
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
    let counts_bytes = pack6(&counts);
    assert!(counts_bytes.len() as u64 == COUNTS_BYTES, "unexpected counts section size");
    drop(counts);

    let mut checksum = FNV_INIT;
    fnv1a64(&mut checksum, &counts_bytes);
    fnv1a64(&mut checksum, &seeds);

    let off_table = HEADER_LEN;
    let off_seeds = off_table + counts_bytes.len() as u64;

    let mut header = [0u8; HEADER_LEN as usize];
    header[0..8].copy_from_slice(&MAGIC);
    header[8..12].copy_from_slice(&FORMAT_CNTS);
    header[12..16].copy_from_slice(&NEW_VERSION.to_le_bytes());
    header[16..24].copy_from_slice(&(total as u64).to_le_bytes());
    header[24..32].copy_from_slice(&off_table.to_le_bytes());
    header[32..40].copy_from_slice(&off_seeds.to_le_bytes());
    header[40..48].copy_from_slice(&checksum.to_le_bytes());

    let mut w = BufWriter::with_capacity(1 << 20, File::create(out).expect("create out"));
    w.write_all(&header).expect("write");
    w.write_all(&counts_bytes).expect("write");
    w.write_all(&seeds).expect("write");
    w.flush().expect("flush");
    eprintln!("phase4 (write) done in {:.1}s", t4.elapsed().as_secs_f64());
    eprintln!(
        "gen-light all done in {:.1}s, {} entries, {} bytes",
        start.elapsed().as_secs_f64(),
        total,
        off_seeds + total as u64 * 2
    );
}

// ---------------------------------------------------------------------------
// 検索

enum Format {
    Cuml,
    Cnts,
}

struct Header {
    format: Format,
    entry_count: u64,
    off_table: u64,
    off_seeds: u64,
    checksum: u64,
}

fn read_header(f: &mut File) -> Header {
    let mut h = [0u8; HEADER_LEN as usize];
    f.read_exact(&mut h).expect("read header");
    assert!(h[0..8] == MAGIC, "bad magic");
    let format = if h[8..12] == FORMAT_CUML {
        Format::Cuml
    } else if h[8..12] == FORMAT_CNTS {
        Format::Cnts
    } else {
        panic!("unsupported format tag: {:?}", &h[8..12]);
    };
    let version = u32::from_le_bytes(h[12..16].try_into().unwrap());
    assert!(version == NEW_VERSION, "unsupported version {}", version);
    let u64at = |o: usize| u64::from_le_bytes(h[o..o + 8].try_into().unwrap());
    let entry_count = u64at(16);
    let off_table = u64at(24);
    let off_seeds = u64at(32);
    let checksum = u64at(40);
    assert!(off_table == HEADER_LEN, "bad table offset");
    let expected_table_bytes = match format {
        Format::Cuml => CUML_BYTES,
        Format::Cnts => COUNTS_BYTES,
    };
    assert!(off_seeds == off_table + expected_table_bytes, "bad seeds offset");
    Header { format, entry_count, off_table, off_seeds, checksum }
}

fn read_at(f: &mut File, off: u64, buf: &mut [u8]) {
    f.seek(SeekFrom::Start(off)).expect("seek");
    f.read_exact(buf).expect("read");
}

fn read_u32_at(f: &mut File, off: u64) -> u32 {
    let mut buf = [0u8; 4];
    read_at(f, off, &mut buf);
    u32::from_le_bytes(buf)
}

/// 観測コード列 c1..c7 で検索し、(起点seed, 7回生成後seed) を返す。
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

    let (lo, hi) = match h.format {
        Format::Cuml => {
            // 累積和表を2箇所seekするだけで区間 [lo, hi) が求まる(ゼロロード)。
            let hi_val = read_u32_at(&mut f, h.off_table + p * 4) as u64;
            let lo_val = if p == 0 {
                0
            } else {
                read_u32_at(&mut f, h.off_table + (p - 1) * 4) as u64
            };
            (lo_val, hi_val)
        }
        Format::Cnts => {
            // 個数列をロードし、先頭からPまでの和で区間 [lo, hi) を得る
            let mut counts_buf = vec![0u8; COUNTS_BYTES as usize];
            read_at(&mut f, h.off_table, &mut counts_buf);
            let (lo, cnt) = scan_prefix(&counts_buf, p as usize);
            (lo, lo + cnt as u64)
        }
    };
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
        // 下位16bitを全探索し、前向きシミュレートで検証
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

// ---------------------------------------------------------------------------
// 検証

/// 構造検証: マジック/フォーマット/バージョン、オフセット整合、ファイル長、
/// 表セクションの整合性(CUML: 単調非減少・最終値==entry_count、CNTS: 総和==entry_count)、
/// チェックサムを確認する。両形式に対応する。
pub fn verify(path: &Path) {
    let mut f = File::open(path).expect("open db");
    let h = read_header(&mut f);
    let flen = f.metadata().expect("meta").len();

    let n = h.entry_count;
    assert!(flen == h.off_seeds + n * 2, "bad file length");

    f.seek(SeekFrom::Start(h.off_table)).expect("seek");
    let mut checksum = FNV_INIT;

    match h.format {
        Format::Cuml => {
            let mut table_buf = vec![0u8; CUML_BYTES as usize];
            f.read_exact(&mut table_buf).expect("read table");
            fnv1a64(&mut checksum, &table_buf);

            let mut prev = 0u32;
            for c in table_buf.chunks_exact(4) {
                let v = u32::from_le_bytes(c.try_into().unwrap());
                assert!(v >= prev, "cumulative table not monotonic");
                prev = v;
            }
            assert!(prev as u64 == n, "cumulative table total != entry_count");
        }
        Format::Cnts => {
            let mut counts_buf = vec![0u8; COUNTS_BYTES as usize];
            f.read_exact(&mut counts_buf).expect("read counts");
            fnv1a64(&mut checksum, &counts_buf);

            let (sum, last) = scan_prefix(&counts_buf, P_COUNT - 1);
            let total = sum + last as u64;
            assert!(total == n, "counts total != entry_count");
        }
    }

    let mut seeds = vec![0u8; (n * 2) as usize];
    f.read_exact(&mut seeds).expect("read seeds");
    fnv1a64(&mut checksum, &seeds);

    assert!(checksum == h.checksum, "checksum mismatch");

    println!("{}: {} entries, OK", path.display(), n);
}

// ---------------------------------------------------------------------------
// 変換 (CUML <-> CNTS、および旧ヘッダ形式(v1/v2/v3)からの取り込み)

/// 入力ファイルを読み、表セクションを共通の個数列(Vec<u32>, 24^5要素)へ正規化して返す。
/// 新ヘッダ(CUML/CNTS)・旧ヘッダ(version 1/2/3)のいずれにも対応する。
/// 戻り値は (個数列, 部分seed配列, entry_count)。
fn load_as_counts(input: &Path) -> (Vec<u32>, Vec<u8>, u64) {
    let mut f = File::open(input).expect("open input");
    let flen = f.metadata().expect("meta").len();

    let mut h = [0u8; HEADER_LEN as usize];
    f.read_exact(&mut h).expect("read header");
    assert!(h[0..8] == MAGIC, "bad magic");
    let u64at = |h: &[u8; HEADER_LEN as usize], o: usize| u64::from_le_bytes(h[o..o + 8].try_into().unwrap());

    let is_new = h[8..12] == FORMAT_CUML || h[8..12] == FORMAT_CNTS;

    // (累積和表か個数列か, entry_count, off_table, 表セクションのバイト長, off_seeds)
    let (is_cuml, entry_count, off_table, table_len, off_seeds) = if is_new {
        let version = u32::from_le_bytes(h[12..16].try_into().unwrap());
        assert!(version == NEW_VERSION, "unsupported new-format version {}", version);
        let is_cuml = h[8..12] == FORMAT_CUML;
        let entry_count = u64at(&h, 16);
        let off_table = u64at(&h, 24);
        let off_seeds = u64at(&h, 32);
        assert!(off_table == HEADER_LEN, "bad table offset");
        let table_len = if is_cuml { CUML_BYTES } else { COUNTS_BYTES };
        assert!(off_seeds == off_table + table_len, "bad seeds offset");
        (is_cuml, entry_count, off_table, table_len, off_seeds)
    } else {
        let ver = u32::from_le_bytes(h[8..12].try_into().unwrap());
        assert!(
            ver == 1 || ver == 2 || ver == 3,
            "unsupported legacy version {}",
            ver
        );
        let entry_count = u64at(&h, 16);
        let off_table = u64at(&h, 24);
        assert!(off_table == HEADER_LEN, "bad table offset");
        // v1: +24 prefix, +32 rems, +40 seeds。v2: +24 prefix, +32 seeds(remsなし)。
        // v3: +24 counts, +32 seeds。
        let (is_cuml, table_len, off_seeds) = if ver == 1 {
            let off_rems = u64at(&h, 32);
            let off_seeds = u64at(&h, 40);
            assert!(off_rems == off_table + CUML_BYTES, "bad rems offset");
            (true, CUML_BYTES, off_seeds)
        } else if ver == 2 {
            let off_seeds = u64at(&h, 32);
            assert!(off_seeds == off_table + CUML_BYTES, "bad seeds offset");
            (true, CUML_BYTES, off_seeds)
        } else {
            let off_seeds = u64at(&h, 32);
            assert!(off_seeds == off_table + COUNTS_BYTES, "bad seeds offset");
            (false, COUNTS_BYTES, off_seeds)
        };
        (is_cuml, entry_count, off_table, table_len, off_seeds)
    };

    assert!(flen == off_seeds + entry_count * 2, "bad input file length");

    let mut table_buf = vec![0u8; table_len as usize];
    f.seek(SeekFrom::Start(off_table)).expect("seek");
    f.read_exact(&mut table_buf).expect("read table");

    let mut seeds = vec![0u8; (entry_count * 2) as usize];
    f.seek(SeekFrom::Start(off_seeds)).expect("seek");
    f.read_exact(&mut seeds).expect("read seeds");
    drop(f);

    // 表セクションを共通の個数列に正規化する(検査も兼ねる)。
    let counts: Vec<u32> = if is_cuml {
        let mut counts = vec![0u32; P_COUNT];
        let mut prev = 0u32;
        for (i, c) in table_buf.chunks_exact(4).enumerate() {
            let v = u32::from_le_bytes(c.try_into().unwrap());
            assert!(v >= prev, "cumulative table not monotonic");
            let diff = v - prev;
            assert!(diff <= 63, "count exceeds 6-bit limit (max 63): {}", diff);
            counts[i] = diff;
            prev = v;
        }
        assert!(prev as u64 == entry_count, "cumulative table total != entry_count");
        counts
    } else {
        let counts = unpack6(&table_buf, P_COUNT);
        let sum: u64 = counts.iter().map(|&c| c as u64).sum();
        assert!(sum == entry_count, "counts total != entry_count");
        counts
    };

    (counts, seeds, entry_count)
}

/// CUML/CNTS(新ヘッダ)、および旧ヘッダ(version 1/2/3)のいずれかのファイルを読み、
/// `target`("cuml" または "cnts")で指定した新ヘッダ形式に変換して書き出す。
/// 累積和→個数は隣接差分(63以下をassert)、個数→累積和は積算で相互変換する。
/// 部分seed配列はそのままコピーし、チェックサムは書き出し時に再計算する。
pub fn convert(input: &Path, output: &Path, target: &str) {
    let start = Instant::now();
    let target_cuml = match target {
        "cuml" => true,
        "cnts" => false,
        _ => panic!("target format must be 'cuml' or 'cnts' (found '{}')", target),
    };

    let (counts, seeds, entry_count) = load_as_counts(input);

    let table_bytes: Vec<u8> = if target_cuml {
        let mut buf = Vec::with_capacity(P_COUNT * 4);
        let mut prev = 0u32;
        for &c in &counts {
            prev += c;
            buf.extend_from_slice(&prev.to_le_bytes());
        }
        buf
    } else {
        pack6(&counts)
    };
    drop(counts);

    let mut checksum = FNV_INIT;
    fnv1a64(&mut checksum, &table_bytes);
    fnv1a64(&mut checksum, &seeds);

    let new_off_table = HEADER_LEN;
    let new_off_seeds = new_off_table + table_bytes.len() as u64;

    let mut header = [0u8; HEADER_LEN as usize];
    header[0..8].copy_from_slice(&MAGIC);
    header[8..12].copy_from_slice(if target_cuml { &FORMAT_CUML } else { &FORMAT_CNTS });
    header[12..16].copy_from_slice(&NEW_VERSION.to_le_bytes());
    header[16..24].copy_from_slice(&entry_count.to_le_bytes());
    header[24..32].copy_from_slice(&new_off_table.to_le_bytes());
    header[32..40].copy_from_slice(&new_off_seeds.to_le_bytes());
    header[40..48].copy_from_slice(&checksum.to_le_bytes());

    let mut w = BufWriter::with_capacity(1 << 20, File::create(output).expect("create out"));
    w.write_all(&header).expect("write");
    w.write_all(&table_bytes).expect("write");
    w.write_all(&seeds).expect("write");
    w.flush().expect("flush");

    let out_len = new_off_seeds + entry_count * 2;
    eprintln!(
        "convert: {} -> {} ({}) done in {:.1}s, {} entries, {} bytes",
        input.display(),
        output.display(),
        target,
        start.elapsed().as_secs_f64(),
        entry_count,
        out_len
    );
}
