//! LightDB圧縮フォーマット(単一ファイル)。仕様はdocs/design-compressed-lightdb.mdを参照。
//! 外部ドキュメントに書いてある内容をここに重複して書く必要はない。
//!
//! キー: 観測1〜7回目のチーム生成コード c1..c7(各 [0,24))に対し
//!   K = c1*(24^6) + c2*(24^5) + ... + c7 (< 24^7、33bit)
//! また、前の5回分だけを取り出した値PをPrefixと呼ぶ(24進数と見れば、前から5文字であるため)。
//!   P = c1*(24^4) + c2*(24^3) + c3*(24^2) + c4*24 + c5
//!
//! 圧縮LightDBはCUML(累積和形式)とCNTS(個数形式)の2種類ある。
//!   - CUML: Prefix P(24^5通り)ごとのエントリ数の累積和(u32)をそのまま並べたテーブル。
//!     オープン時のロードが不要で、検索は表を2箇所seekするだけで完結する(表は31.85MB)。
//!   - CNTS: Prefix Pごとのエントリ数を、中心値16からの残差のzigzag+Rice符号で
//!     エンコードした個数列。ファイルサイズが最小になる代わりに、オープン時に個数列を
//!     ロードし検索のたびに先頭からの復号(累積和相当)を構築する必要がある(表は約4MB)。
//!
//! ファイルレイアウト(すべてリトルエンディアン、両形式共通):
//!   offset 0: ヘッダ64B
//!     +0  magic    [u8; 8] = "COLIGHT\0" (ファミリー識別、両形式で不変)
//!     +8  format   [u8; 4] = "CUML" または "CNTS" (FourCC、ASCII)
//!     +12 version  u32 = 1 (形式ごとに1から振り直す)
//!     +16 entry_count u64
//!     +24 表セクションオフセットu64 (= 64。CUMLなら累積和表、CNTSなら個数列)
//!     +32 部分seed配列オフセットu64
//!         (CUML: 64 + 24^5*4 = 31,850,560)
//!         (CNTS: 64 + 個数列セクション長(可変、8Bアラインされる))
//!     +40 checksum u64 (表セクション+部分seed配列のFNV-1a 64、この順)
//!     +48 Rice符号化パラメータk (u8)。CNTSのみ使用、生成時にファイル全体が
//!         最小になる値を選んで記録する(CUMLでは未使用、0のまま)
//!     +49〜63 (予約) = 0
//!   表セクション:
//!     CUML: Prefix P(24^5通り)ごとのエントリ数の累積和を先頭からu32(LE)で並べたテーブル。
//!           要素iの値は「P<=iであるエントリ数の総和」。
//!     CNTS: Prefix Pごとのエントリ数countについて、残差r = count - 16をzigzagで非負整数化した
//!           z = (r << 1) ^ (r >> 31)を、パラメータkでRice符号化(商z>>kを「1がq個+0」の
//!           unary、剰余の下位kbitをLSBファースト)した列を先頭から並べる。ビット列は
//!           LSBファーストでバイト詰めし、末尾は8B境界までゼロ詰めする。
//!   部分seed配列:エントリのK昇順に、観測1回目開始時点のseedの上位16bitを並べたテーブル
//!     (両形式共通)。
//!
//! キーはエントリのソートに使う。c1..c5から計算されるPrefixは表セクションに反映されるが
//! (値そのものはファイルに保存されない)、c6・c7はソートにのみ使われる。
//! 検索は、CUMLなら表を2箇所seekして区間[lo, hi)を直接得る。CNTSなら個数列をロードし
//! 先頭からPまでをRice復号しながら和を取ることで区間開始を求め、区間終了は開始+個数[P]で得る。
//! 区間内(平均16.1エントリ)の全エントリについて部分seedの下位16bitを全探索し、7回分のコードを
//! 入力値と照合(`generate_team_checked`)して、完全一致するものだけを返す。
//!

use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use crate::teamgen::{self, generate_team, generate_team_checked};

pub const MAGIC: [u8; 8] = *b"COLIGHT\0";
const FORMAT_CUML: [u8; 4] = *b"CUML";
const FORMAT_CNTS: [u8; 4] = *b"CNTS";
const NEW_VERSION: u32 = 1;
const P_COUNT: usize = 7_962_624; // 24^5
const HEADER_LEN: u64 = 64;
const CUML_BYTES: u64 = P_COUNT as u64 * 4; // 31,850,496
const COUNT_CENTER: i32 = 16; // 個数列の残差符号化における中心値(固定)

fn fnv1a64(h: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *h ^= b as u64;
        *h = h.wrapping_mul(0x100_0000_01b3);
    }
}
const FNV_INIT: u64 = 0xcbf2_9ce4_8422_2325;

// ---------------------------------------------------------------------------
// 個数列の符号化(残差のzigzag化 + Rice符号)

/// 残差r(= count - 16)をzigzagマッピングで非負整数化する(0,-1,1,-2,... → 0,1,2,3,...)。
fn zigzag_encode(r: i32) -> u32 {
    ((r << 1) ^ (r >> 31)) as u32
}

/// zigzagマッピングされた値zを残差rに戻す。
fn zigzag_decode(z: u32) -> i32 {
    ((z >> 1) as i32) ^ -((z & 1) as i32)
}

/// LSBファーストでビットを詰めていくビットライタ。
struct BitWriter {
    buf: Vec<u8>,
    acc: u64,
    nbits: u32,
}

impl BitWriter {
    fn new() -> Self {
        Self { buf: Vec::new(), acc: 0, nbits: 0 }
    }

    /// `value`の下位`n`bitをLSBファーストで書き出す(n <= 32)。
    fn put_bits(&mut self, value: u32, n: u32) {
        self.acc |= (value as u64) << self.nbits;
        self.nbits += n;
        while self.nbits >= 8 {
            self.buf.push((self.acc & 0xFF) as u8);
            self.acc >>= 8;
            self.nbits -= 8;
        }
    }

    /// unary符号(1がq個 + 終端の0)を書き出す。
    fn put_unary(&mut self, mut q: u32) {
        while q >= 24 {
            self.put_bits(0x00FF_FFFF, 24);
            q -= 24;
        }
        self.put_bits((1u32 << q) - 1, q + 1);
    }

    /// 端数ビットをバイトに書き出したのち、8バイト境界までゼロ詰めして返す。
    fn finish(mut self) -> Vec<u8> {
        if self.nbits > 0 {
            self.buf.push((self.acc & 0xFF) as u8);
        }
        while self.buf.len() % 8 != 0 {
            self.buf.push(0);
        }
        self.buf
    }
}

/// LSBファーストでビットを読み出すビットリーダ。
struct BitReader<'a> {
    buf: &'a [u8],
    byte_idx: usize,
    bit_idx: u32,
}

impl<'a> BitReader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, byte_idx: 0, bit_idx: 0 }
    }

    fn read_bit(&mut self) -> u32 {
        let b = (self.buf[self.byte_idx] >> self.bit_idx) & 1;
        self.bit_idx += 1;
        if self.bit_idx == 8 {
            self.bit_idx = 0;
            self.byte_idx += 1;
        }
        b as u32
    }

    /// 1が続く数(unary符号の商q)を終端の0まで読む。
    fn read_unary(&mut self) -> u32 {
        let mut q = 0u32;
        while self.read_bit() == 1 {
            q += 1;
        }
        q
    }

    fn read_bits(&mut self, n: u32) -> u32 {
        let mut v = 0u32;
        for i in 0..n {
            v |= self.read_bit() << i;
        }
        v
    }
}

/// 1個の個数をRice符号化してビットライタに書き出す。
fn rice_encode_one(w: &mut BitWriter, count: u32, k: u8) {
    let r = count as i32 - COUNT_CENTER;
    let z = zigzag_encode(r);
    let q = z >> k;
    w.put_unary(q);
    if k > 0 {
        w.put_bits(z & ((1u32 << k) - 1), k as u32);
    }
}

/// ビットリーダから1個の個数をRice復号する。
fn rice_decode_one(r: &mut BitReader, k: u8) -> u32 {
    let q = r.read_unary();
    let rem = if k > 0 { r.read_bits(k as u32) } else { 0 };
    let z = (q << k) | rem;
    (COUNT_CENTER + zigzag_decode(z)) as u32
}

/// 個数列をパラメータkでRice符号化し、8B境界までゼロ詰めしたバイト列を返す。
fn rice_pack(counts: &[u32], k: u8) -> Vec<u8> {
    let mut w = BitWriter::new();
    for &c in counts {
        rice_encode_one(&mut w, c, k);
    }
    w.finish()
}

/// Rice符号化された個数列を先頭から`count`個復号する。
fn rice_unpack(buf: &[u8], k: u8, count: usize) -> Vec<u32> {
    let mut r = BitReader::new(buf);
    (0..count).map(|_| rice_decode_one(&mut r, k)).collect()
}

/// Rice符号化された個数列を先頭から`p`番目まで走査し、
/// (個数[0..p]の総和, 個数[p])を返す。
fn rice_scan_prefix(buf: &[u8], k: u8, p: usize) -> (u64, u32) {
    let mut r = BitReader::new(buf);
    let mut sum = 0u64;
    let mut val = 0u32;
    for i in 0..=p {
        let v = rice_decode_one(&mut r, k);
        if i == p {
            val = v;
        } else {
            sum += v as u64;
        }
    }
    (sum, val)
}

/// Rice符号化後の全体サイズ(概算ビット数)が最小になるkを選ぶ。
fn choose_optimal_k(counts: &[u32]) -> u8 {
    let zs: Vec<u32> = counts.iter().map(|&c| zigzag_encode(c as i32 - COUNT_CENTER)).collect();
    let mut best_k = 0u8;
    let mut best_bits = u64::MAX;
    for k in 0u8..=12 {
        let bits: u64 = zs.iter().map(|&z| (z >> k) as u64 + 1 + k as u64).sum();
        if bits < best_bits {
            best_bits = bits;
            best_k = k;
        }
    }
    best_k
}

/// (K, 7回生成後seed, 起点seed)。Kは33bitなので上位をe[0](0/1)、下位をe[1]に分ける。
/// e[2] = 7回生成後seed、e[3] = 起点seed。[u32; 4]の辞書式順序でソートすることで
/// K → 7回生成後seed → 起点seedの順に整列される。
type Entry = [u32; 4];

// ---------------------------------------------------------------------------
// 生成: LCG軌道順走査 + スライディングウィンドウ
//
// 設計の詳細・根拠はdocs/design-orbit-scan.mdを参照。

/// リングバッファの物理スロット数。論理位置pのスロットはp & (RING-1)。
const RING: usize = 1 << 18;
const RING_MASK: u64 = (RING as u64) - 1;
/// 解決までの遅延。走査位置がj+LAGに達した時点で位置jを解決する。
const LAG: i64 = 1 << 17;
/// 弧の前段で先行走査する位置数。位置jの像フラグを立てうる前任位置iは
/// j-PROLOGUEより大きくj以下に限られる(1回のチーム生成の消費数nがu16に収まることから保証される)。
const PROLOGUE: i64 = 1 << 16;
/// 状態列(ring.s)の書き込みを生成位置より先行させる位置数。
/// 平均消費数n(約1,265)に対して十分小さく取れば、fill(write)と生成(read)が時間的に
/// 近接しキャッシュヒットしやすくなる。ただしn>LEADの位置は生成が書き込み位置を追い越して
/// 古いデータを読むため、`teamgen::generate_team_from_table`がNoneを返し、その位置だけ
/// スカラー(`generate_team_with_count`)で再計算する(フォールバック)。
/// 安全性は`LEAD < RING-LAG`(= 2^17)にのみ依存し、小さくするほど安全側(詳細はRing構造体の
/// ドキュメントコメントおよびdocs/design-orbit-scan.md参照)。
const LEAD: i64 = 1 << 14;

/// リングバッファ本体(SoA)。論理位置pのスロットにコードc(u8)・消費数n(u16)・
/// 状態s(u32、その位置での軌道の状態そのもの)・像フラグを持つ。
///
/// スロット再利用の安全性: 物理スロット(p & (RING-1))は論理位置
/// p, p+RING, p+2*RING, ... で使い回される。以下の順序が常に成り立つように
/// 定数を選んである(PROLOGUE < RING-LAGが根拠。RING=2^18, LAG=2^17,
/// PROLOGUE=2^16なのでRING-LAG=2^17 > PROLOGUE=2^16):
///   「位置p-RINGの解決時クリア(時刻(p-RING)+LAG = p-(RING-LAG))」
///   → 「pへの像フラグ書き込み(最速で時刻p-PROLOGUE)」
///   → 「pの走査(時刻p)」
///   → 「pの解決(時刻p+LAG)」
/// したがって新しい論理位置pのためのフラグ書き込みは、必ず前の使用者(p-RING)の
/// クリアより後に起こり、フラグが混線することはない。
///
/// s配列のみ、位置p+LEADへの先行書き込みで論理位置p+LEAD-RING(= p-(RING-LEAD))の
/// sを上書きする。しかし解決時のホップ参照が読むsは[p-LAG, p]の範囲に限られ、
/// LEAD < RING-LAG(= 2^17)である限りp+LEAD-RING < p-LAGなので混線しない
/// (生きているsの範囲は[p+LEAD-RING, p+LEAD]で、LEADを小さくするほど安全側)。
/// hiは各スロットのsの上位16bit(=その位置での乱数出力)を別配列で持ったもの。
/// チーム生成(generate_team_from_table)はこのhiだけを読む。u32のsではなくu16の
/// hiを読むことで、ホットループの読み込みウィンドウ(約LEAD要素)が半分のバイト数に
/// なりL1Dに収まりやすくなる(帯域も半減する)。sとhiは常に同じ位置で同時に書き込む。
struct Ring {
    c: Vec<u8>,
    n: Vec<u16>,
    s: Vec<u32>,
    hi: Vec<u16>,
    img: Vec<bool>,
}

impl Ring {
    fn new() -> Self {
        Self {
            c: vec![0u8; RING],
            n: vec![0u16; RING],
            s: vec![0u32; RING],
            hi: vec![0u16; RING],
            img: vec![false; RING],
        }
    }

    #[inline(always)]
    fn idx(pos: i64) -> usize {
        (pos as u64 & RING_MASK) as usize
    }
}

/// 位置startからリングバッファ内を7ホップ(start→start+n→…)辿り、
/// キーK(7個のコードを24進数として結合した値)と7回チーム生成後のseedを合成する。
/// ホップ先がscanned_upto(この時点で走査済みの末尾位置)を超える場合は、
/// startの状態s_startから素直にチーム生成を7回呼び直すフォールバックで解決する
/// (平均ホップ距離は約8,850でLAGに対し十分小さいため、実質発生しない)。
fn hop_key(ring: &Ring, start: i64, scanned_upto: i64) -> (u64, u32) {
    let mut pos = start;
    let mut k: u64 = 0;
    for _ in 0..7 {
        if pos > scanned_upto {
            return hop_key_fallback(ring, start);
        }
        let slot = Ring::idx(pos);
        k = k * 24 + ring.c[slot] as u64;
        pos += ring.n[slot] as i64;
    }
    if pos > scanned_upto {
        return hop_key_fallback(ring, start);
    }
    (k, ring.s[Ring::idx(pos)])
}

fn hop_key_fallback(ring: &Ring, start: i64) -> (u64, u32) {
    let mut s = ring.s[Ring::idx(start)];
    let mut k: u64 = 0;
    for _ in 0..7 {
        k = k * 24 + generate_team(&mut s) as u64;
    }
    (k, s)
}

/// 軌道上の弧[arc_start, arc_end)を走査・解決し、生成したエントリをc1(0..24)ごとの
/// バケツに詰めて返す。弧の前段PROLOGUE個の位置(像フラグの伝播のみ)と後段LAG個の位置
/// (弧内の全位置が解決されるまでのホップ参照用)も合わせて走査するが、
/// エントリを実際に作るのは解決した位置がarc_start以上arc_end未満のときだけ
/// (前後の弧との重複走査分は、隣接する弧自身がそれぞれ責任を持つ範囲でのみemitする)。
fn scan_arc(arc_start: i64, arc_end: i64, self_checks: &AtomicU64, fallbacks: &AtomicU64) -> Vec<Vec<Entry>> {
    let mut buckets: Vec<Vec<Entry>> = (0..24).map(|_| Vec::new()).collect();
    let mut local_checks = 0u64;
    let mut local_fallbacks = 0u64;

    let scan_start = arc_start - PROLOGUE;
    let scan_end = arc_end + LAG; // 排他的上限

    // 弧の起点状態は、軌道の起点(seed 0)からのLCGジャンプで直接求める。
    let start_off = scan_start.rem_euclid(1i64 << 32) as u32;
    let mut cur_s = teamgen::lcg_jump(0, start_off);
    let mut ring = Ring::new();

    // 状態列の先行書き込み: 生成位置が位置pにいるとき、ring.sは位置p+LEADまで
    // 書き込み済みであるようにする(チーム生成はring.s読みのテーブル駆動で行うため)。
    // まず[scan_start, scan_start+LEAD)を埋め、以降はループ内で1位置ずつ先へ埋める。
    for q in scan_start..scan_start + LEAD {
        let slot = Ring::idx(q);
        ring.s[slot] = cur_s;
        ring.hi[slot] = (cur_s >> 16) as u16;
        cur_s = teamgen::step(cur_s);
    }

    for p in scan_start..scan_end {
        // 位置p+LEADの状態を書き込む(cur_sは常に書き込み位置の状態)
        let wslot = Ring::idx(p + LEAD);
        ring.s[wslot] = cur_s;
        ring.hi[wslot] = (cur_s >> 16) as u16;
        cur_s = teamgen::step(cur_s);

        let slot = Ring::idx(p);
        let s_here = ring.s[slot];

        // 生成位置: 位置pで1回だけチーム生成する。乱数値は書き込み済みのring.sから
        // テーブル駆動で読むため、LCGの逐次乗算チェーンを含まない。
        // LEADを追い越す(n>LEAD)稀な位置ではNoneが返るので、その位置だけ
        // スカラー(逐次LCG)で再計算する。
        let (code, n) = match teamgen::generate_team_from_table(&ring.hi, slot, RING - 1, LEAD as usize) {
            Some(r) => r,
            None => {
                local_fallbacks += 1;
                let mut s = s_here;
                teamgen::generate_team_with_count(&mut s)
            }
        };
        assert!(
            n <= u16::MAX as u32,
            "team generation consumed {} rand calls (> u16::MAX) at s=0x{:08X}",
            n,
            s_here
        );
        ring.c[slot] = code as u8;
        ring.n[slot] = n as u16;
        // 像フラグを位置p+nのスロットに立てる(そのスロットのc/n/sは未書き込みのまま)
        ring.img[Ring::idx(p + n as i64)] = true;

        // 位置p-LAGを解決する
        let resolve_p = p - LAG;
        if resolve_p >= scan_start {
            let rslot = Ring::idx(resolve_p);
            let is_image = ring.img[rslot];
            ring.img[rslot] = false; // その場でクリアし、スロット再利用に備える
            if is_image {
                let origin = ring.s[rslot];
                let (k, final_seed) = hop_key(&ring, resolve_p, p);

                // オンライン自己検査: 全周期で約100万サンプル、決定的、常時有効
                if origin & 0xFFF == 0 {
                    local_checks += 1;
                    let mut s = origin;
                    let mut kk: u64 = 0;
                    for _ in 0..7 {
                        kk = kk * 24 + generate_team(&mut s) as u64;
                    }
                    assert!(
                        kk == k && s == final_seed,
                        "self-check failed at origin=0x{:08X}: window(K={}, final=0x{:08X}) != fresh(K={}, final=0x{:08X})",
                        origin,
                        k,
                        final_seed,
                        kk,
                        s
                    );
                }

                if resolve_p >= arc_start && resolve_p < arc_end {
                    let c1 = (k / 24u64.pow(6)) as usize;
                    buckets[c1].push([(k >> 32) as u32, k as u32, final_seed, origin]);
                }
            }
        }
    }

    self_checks.fetch_add(local_checks, Ordering::Relaxed);
    fallbacks.fetch_add(local_fallbacks, Ordering::Relaxed);
    buckets
}

pub fn generate(out: &Path, limit: u64, threads: usize) {
    let start = Instant::now();
    eprintln!(
        "gen-light: positions=0x{:X} (seed 0を起点とする軌道の先頭limit位置), threads={}",
        limit, threads
    );

    // Phase 1+2: LCG軌道順走査+スライディングウィンドウで、像判定とキー合成を
    // 同時に行い、直接エントリを生成する。c1 (= K / 24^6) で24バケツに分ける。
    let t12 = Instant::now();
    let self_checks = AtomicU64::new(0);
    let fallbacks = AtomicU64::new(0);
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
                let self_checks = &self_checks;
                let fallbacks = &fallbacks;
                let results = &results;
                sc.spawn(move || {
                    let local = scan_arc(a0, a1, self_checks, fallbacks);
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
    // 総生成回数=各弧のscan区間長(arc_len+PROLOGUE+LAG)の総和=limit+threads*(PROLOGUE+LAG)
    let total_gens = limit + threads_u * (PROLOGUE + LAG) as u64;
    let fallback_n = fallbacks.load(Ordering::Relaxed);
    eprintln!(
        "phase1+2 (orbit scan) done in {:.1}s, entries={}, self-checks={}, fallbacks={} ({:.4}%)",
        t12.elapsed().as_secs_f64(),
        total,
        self_checks.load(Ordering::Relaxed),
        fallback_n,
        100.0 * fallback_n as f64 / total_gens as f64
    );

    // Phase 3: バケツごとに[u32; 4]の辞書式順(K→7回生成後seed→起点seed)にソートし、
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
    let rice_k = choose_optimal_k(&counts);
    let counts_bytes = rice_pack(&counts, rice_k);
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
    header[48] = rice_k;

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
    rice_k: u8,
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
    let rice_k = h[48];
    assert!(off_table == HEADER_LEN, "bad table offset");
    match format {
        Format::Cuml => {
            assert!(off_seeds == off_table + CUML_BYTES, "bad seeds offset");
        }
        Format::Cnts => {
            assert!((off_seeds - off_table) % 8 == 0, "counts section not 8B-aligned");
        }
    }
    Header { format, entry_count, off_table, off_seeds, checksum, rice_k }
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

    let (lo, hi) = match h.format {
        Format::Cuml => {
            // 累積和表を2箇所seekするだけで区間[lo, hi)が求まる(ゼロロード)。
            let hi_val = read_u32_at(&mut f, h.off_table + p * 4) as u64;
            let lo_val = if p == 0 {
                0
            } else {
                read_u32_at(&mut f, h.off_table + (p - 1) * 4) as u64
            };
            (lo_val, hi_val)
        }
        Format::Cnts => {
            // 個数列をロードし、先頭からPまでRice復号した和で区間[lo, hi)を得る
            let table_len = (h.off_seeds - h.off_table) as usize;
            let mut counts_buf = vec![0u8; table_len];
            read_at(&mut f, h.off_table, &mut counts_buf);
            let (lo, cnt) = rice_scan_prefix(&counts_buf, h.rice_k, p as usize);
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
            let table_len = (h.off_seeds - h.off_table) as usize;
            let mut counts_buf = vec![0u8; table_len];
            f.read_exact(&mut counts_buf).expect("read counts");
            fnv1a64(&mut checksum, &counts_buf);

            let (sum, last) = rice_scan_prefix(&counts_buf, h.rice_k, P_COUNT - 1);
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
// 変換(CUML <-> CNTS)

/// 入力ファイルを読み、表セクションを共通の個数列(Vec<u32>, 24^5要素)へ正規化して返す。
/// 戻り値は(個数列, 部分seed配列, entry_count)。
fn load_as_counts(input: &Path) -> (Vec<u32>, Vec<u8>, u64) {
    let mut f = File::open(input).expect("open input");
    let flen = f.metadata().expect("meta").len();

    let mut h = [0u8; HEADER_LEN as usize];
    f.read_exact(&mut h).expect("read header");
    assert!(h[0..8] == MAGIC, "bad magic");
    let u64at = |h: &[u8; HEADER_LEN as usize], o: usize| u64::from_le_bytes(h[o..o + 8].try_into().unwrap());

    let is_cuml = if h[8..12] == FORMAT_CUML {
        true
    } else if h[8..12] == FORMAT_CNTS {
        false
    } else {
        panic!("unsupported format tag: {:?}", &h[8..12]);
    };
    let version = u32::from_le_bytes(h[12..16].try_into().unwrap());
    assert!(version == NEW_VERSION, "unsupported version {}", version);
    let entry_count = u64at(&h, 16);
    let off_table = u64at(&h, 24);
    let off_seeds = u64at(&h, 32);
    let rice_k = h[48];
    assert!(off_table == HEADER_LEN, "bad table offset");
    let table_len = if is_cuml { CUML_BYTES } else { off_seeds - off_table };
    assert!(off_seeds == off_table + table_len, "bad seeds offset");

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
            counts[i] = v - prev;
            prev = v;
        }
        assert!(prev as u64 == entry_count, "cumulative table total != entry_count");
        counts
    } else {
        let counts = rice_unpack(&table_buf, rice_k, P_COUNT);
        let sum: u64 = counts.iter().map(|&c| c as u64).sum();
        assert!(sum == entry_count, "counts total != entry_count");
        counts
    };

    (counts, seeds, entry_count)
}

/// CUML/CNTSいずれかのファイルを読み、`target`("cuml" または "cnts")で指定した形式に
/// 変換して書き出す。累積和→個数は隣接差分、個数→累積和は積算で相互変換する。
/// 部分seed配列はそのままコピーし、チェックサムは書き出し時に再計算する。
pub fn convert(input: &Path, output: &Path, target: &str) {
    let start = Instant::now();
    let target_cuml = match target {
        "cuml" => true,
        "cnts" => false,
        _ => panic!("target format must be 'cuml' or 'cnts' (found '{}')", target),
    };

    let (counts, seeds, entry_count) = load_as_counts(input);

    let (table_bytes, rice_k): (Vec<u8>, u8) = if target_cuml {
        let mut buf = Vec::with_capacity(P_COUNT * 4);
        let mut prev = 0u32;
        for &c in &counts {
            prev += c;
            buf.extend_from_slice(&prev.to_le_bytes());
        }
        (buf, 0)
    } else {
        let k = choose_optimal_k(&counts);
        (rice_pack(&counts, k), k)
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
    header[48] = rice_k;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// std::env::temp_dir()配下に、プロセスID+ナノ秒時刻でユニークなパスを作る
    /// (並列テスト実行や再実行での衝突を避ける)。
    fn unique_temp_path(tag: &str) -> PathBuf {
        let pid = std::process::id();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("codb-gen-test-{}-{}-{}.cldb", tag, pid, nanos))
    }

    /// 生成の並列度不変性・オンライン自己検査・構造検証を1本で確認する。
    /// - `generate(&p1, LIMIT, 1)` と `generate(&p4, LIMIT, 4)` を実行し、
    ///   出力バイト列が完全一致すること(スレッド数がバイト出力に影響しないこと)を確認する。
    /// - 生成の実行中、gen-light内蔵のオンライン自己検査(scan_arc内、
    ///   テーブル駆動の結果をスカラー再計算と照合)が走り、不一致ならpanicする。
    ///   すなわちこのテスト自体が自己検査を駆動する。
    /// - `verify(&p1)`がpanicしないこと(構造検証: ヘッダ・オフセット・チェックサム等)を確認する。
    #[test]
    fn thread_invariance_and_selfcheck() {
        // limitに対して固定のスキャン前後幅(PROLOGUE=2^16+LAG=2^17=196,608位置/スレッド)が
        // 支配的なため、limitを小さくしても際限なく速くなるわけではない。0x8000(=32768)は
        // 自己検査サンプル(origin下位12bit==0、期待値約8件)が実効的に発生しつつ、
        // cargo test全体を数秒〜十数秒に収めるために選んだ値。
        const LIMIT: u64 = 0x8000;

        let p1 = unique_temp_path("t1");
        let p4 = unique_temp_path("t4");

        generate(&p1, LIMIT, 1);
        generate(&p4, LIMIT, 4);

        let b1 = std::fs::read(&p1).expect("read p1 output");
        let b4 = std::fs::read(&p4).expect("read p4 output");
        assert_eq!(b1, b4, "generate() output must be byte-identical regardless of thread count");

        // 構造検証(マジック/フォーマット/オフセット/チェックサム等)がpanicしないこと。
        verify(&p1);

        std::fs::remove_file(&p1).ok();
        std::fs::remove_file(&p4).ok();
    }
}
