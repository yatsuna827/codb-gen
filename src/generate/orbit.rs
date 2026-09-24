use crate::core::lcg;
use crate::core::teamgen::generate_team;

use super::teamgen_optimized::{generate_team_from_table, make_cond_code, CondCode};
use super::Entry;

/// `[arc_start, arc_end)`を走査して、先頭のコード(c1)でバケット分けしたEntryのリストを返す
pub(super) fn scan_arc(arc_start: i64, arc_end: i64) -> Vec<Vec<Entry>> {
    let mut buckets: Vec<Vec<Entry>> = (0..24).map(|_| Vec::new()).collect();

    // 十分前から走査を始めて、`[arc_start, arc_end)`に含まれるseedに対するImg判定が尽くされるようにする
    let scan_start = arc_start - PROLOGUE;
    // 十分先まで走査して、`[arc_start, arc_end)`に含まれるseedから7回分のチーム生成結果がringに書き込まれるようにする
    let scan_end = arc_end + LAG;

    let mut seed_lead = lcg::lcg_jump(0, scan_start.rem_euclid(1i64 << 32) as u32);
    let mut seed_resolve = lcg::lcg_jump(0, (scan_start - LAG).rem_euclid(1i64 << 32) as u32);
    let mut prev_rand: u16 = 0;

    let mut ring = Ring::new();

    // NOTE: 生成位置が位置`p`にいるとき、`ring.rand`は位置`p+LEAD`まで、
    // `ring.code`は位置`p+LEAD-1`まで書き込まれていることを保証する

    // 先にrangとcodeを充填しておく
    for q in scan_start..scan_start + LEAD {
        let rand = (seed_lead >> 16) as u16;
        ring.rand[Ring::idx(q)] = rand;
        // 初回はprev_randがないので充填できない
        if q > scan_start {
            ring.put_cond_code(q - 1, make_cond_code(prev_rand, rand));
        }
        prev_rand = rand;
        seed_lead = lcg::adv(seed_lead);
    }

    for p in scan_start..scan_end {
        let rand = (seed_lead >> 16) as u16;
        ring.rand[Ring::idx(p + LEAD)] = rand;
        ring.put_cond_code(p + LEAD - 1, make_cond_code(prev_rand, rand));
        prev_rand = rand;
        seed_lead = lcg::adv(seed_lead);

        // NOTE: 各消費位置でのチーム生成処理は、ここで1回だけ行われる。
        // 乱数値は事前に計算済みのものをringから読み出して使うことで、
        // （乗算を含む）LCG遷移関数の呼び出し回数を大幅にカットしている。

        let slot = Ring::idx(p);
        let (code, adv) = generate_team_from_table(
            &ring.rand,
            [&ring.cond_code[0], &ring.cond_code[1]],
            slot,
            RING_BUF_SIZE - 1,
            LEAD as usize,
        );
        assert!(
            adv <= u16::MAX as u32,
            "team generation consumed {} rand calls (> u16::MAX) at s=0x{:08X}",
            adv,
            lcg::lcg_jump(seed_resolve, LAG as u32)
        );
        ring.code[slot] = code as u8;
        ring.adv[slot] = adv as u16;
        ring.img[Ring::idx(p + adv as i64)] = true;

        resolve_at(
            &mut ring,
            p - LAG,
            seed_resolve,
            scan_start,
            arc_start,
            arc_end,
            &mut buckets,
        );

        seed_resolve = lcg::adv(seed_resolve);
    }

    buckets
}

// 位置`p`から7回分のチーム生成がringに書き込まれるまで待つウィンドウのサイズ
pub(super) const LAG: i64 = 1 << 17;

// 位置`p`がImgに含まれるかどうかが調べ尽くされるまで待つウィンドウのサイズ
pub(super) const PROLOGUE: i64 = 1 << 16;

// 先んじてringにrandとcodeを充填するために先行して処理するウィンドウのサイズ
// 小さいほどwriteとreadが時間的に近くなってキャッシュヒットしやすくなるが、
// 小さくしすぎると充填量が不足する
const LEAD: i64 = 1 << 14;

const RING_BUF_SIZE: usize = 1 << 18;
const RING_BUF_MASK: u64 = (RING_BUF_SIZE as u64) - 1;

// 各種リングバッファを束ねたやつ
struct Ring {
    /// 位置[p]で生成される1回分のコード
    code: Vec<u8>,
    /// 位置[p]での1回のチーム生成処理の合計消費数
    adv: Vec<u16>,
    /// 位置[p]で生成される乱数値
    rand: Vec<u16>,
    /// 位置[p]で生成される性格値のCondCode \
    /// SIMDロード効率化のためにpの偶奇で分けて保持する
    cond_code: [Vec<CondCode>; 2],
    /// 位置[p]のseedが生成関数を1回通した像(Image)に含まれるseedかどうか
    img: Vec<bool>,
}
impl Ring {
    fn new() -> Self {
        Self {
            code: vec![0u8; RING_BUF_SIZE],
            adv: vec![0u16; RING_BUF_SIZE],
            rand: vec![0u16; RING_BUF_SIZE],
            cond_code: [vec![0u8; RING_BUF_SIZE / 2], vec![0u8; RING_BUF_SIZE / 2]],
            img: vec![false; RING_BUF_SIZE],
        }
    }

    #[inline(always)]
    fn idx(pos: i64) -> usize {
        (pos as u64 & RING_BUF_MASK) as usize
    }

    #[inline(always)]
    fn put_cond_code(&mut self, pos: i64, value: CondCode) {
        // NOTE: u64へのキャストは偶奇およびRing::idxとの可換性を保つ
        let q = pos as u64;
        self.cond_code[(q & 1) as usize][((q >> 1) & (RING_BUF_MASK >> 1)) as usize] = value;
    }
}

#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn resolve_at(
    ring: &mut Ring,
    idx: i64,
    seed: u32,
    scan_start: i64,
    arc_start: i64,
    arc_end: i64,
    buckets: &mut [Vec<Entry>],
) {
    if idx < scan_start {
        return;
    }

    let rslot = Ring::idx(idx);
    let is_in_image = ring.img[rslot];
    ring.img[rslot] = false;
    if !is_in_image {
        return;
    }

    let (code, final_seed) = gen_code(ring, idx, seed);

    // NOTE: 最適化のためにリングバッファを再利用しているが、パラメータ次第で
    // 計算結果が狂いかねないため、サンプリングして検証している
    // 検証の有無で実行時間に有意な差は見られなかったので本番ビルドでも残してある
    if seed & 0xFFF == 0 {
        let mut s = seed;
        let mut kk: u64 = 0;
        for _ in 0..7 {
            kk = kk * 24 + generate_team(&mut s) as u64;
        }
        assert!(
            kk == code && s == final_seed,
            "self-check failed at seed=0x{:08X}: window(K={}, final=0x{:08X}) != fresh(K={}, final=0x{:08X})",
            seed,
            code,
            final_seed,
            kk,
            s
        );
    }

    // NOTE: 実際に計算すべき`[arc_start, arc_end)`の外側まで処理されるので、
    // 範囲外の位置に対する計算結果は戻り値に含めないようにする必要がある
    if arc_start <= idx && idx < arc_end {
        let c1 = (code / 24u64.pow(6)) as usize;
        buckets[c1].push([(code >> 32) as u32, code as u32, final_seed, seed]);
    }
}

/// 7回分の生成処理を計算して(コード,計算後のseed)を返す
fn gen_code(ring: &Ring, start: i64, seed: u32) -> (u64, u32) {
    // NOTE: ringに蓄積されている計算済みの値はscanned_uptoの位置まで信頼できる
    // 7回分の生成処理の合計消費数はTSV指定なしの場合で最大15525と、LAGに対して十分小さいので、これを超えることはない

    let mut pos = start;
    let mut k: u64 = 0;
    for _ in 0..7 {
        let slot = Ring::idx(pos);
        k = k * 24 + ring.code[slot] as u64;
        pos += ring.adv[slot] as i64;
    }

    let final_seed = lcg::lcg_jump(seed, (pos - start) as u32);
    (k, final_seed)
}
