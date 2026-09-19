//! ポケモンコロシアムの「とにかくバトル」の「シングル・最強」のチーム生成処理
//! こっちはいろいろ最適化が施されている

use crate::core::teamdef::{gender_bucket, Slot, TEAMS};

pub(super) type CondCode = u8;

#[inline(always)]
pub fn make_cond_code(hid: u16, lid: u16) -> CondCode {
    let pid = ((hid as u32) << 16) | lid as u32;
    let nature = (pid % 25) as u8;
    nature * 5 + gender_bucket((lid & 0xFF) as u32)
}

/// 事前計算済みの乱数値列のリングバッファから乱数値を読んで1回分のチーム生成を行う。
/// 戻り値は `(コード, 消費数)`。leadまでの範囲内に候補が見つからなければpanicする。
#[inline(always)]
pub fn generate_team_from_table(
    table: &[u16], // あらかじめ計算された乱数値の配列
    // MEMO: 性格値再計算は1回に2消費ずつ進むので、個体の候補は1つおきに並ぶ。
    // そのため、奇数キーと偶数キーに分けて並べることで、SIMDロードが効率化される。
    keys: [&[CondCode]; 2], // 位置iに対応するcond_codeは`keys[i & 1][(i & mask) >> 1]`に格納されている
    start_idx: usize,
    mask: usize,
    lead: usize, // 呼び出し側での書き込み位置が`start_idx`基準でどれだけ進んでいるか
) -> (u32, u32) {
    debug_assert!(mask < table.len());
    debug_assert!(keys[0].len() == table.len() / 2 && keys[1].len() == table.len() / 2);

    let idx_limit = start_idx + lead;
    let mut idx = start_idx;
    let mut n = 0u32;

    // 相手チーム決定
    let e = (next_rand_tbl(table, mask, &mut idx) & 7) as usize;
    n += 1;
    // 自チーム決定
    let p = loop {
        let p = (next_rand_tbl(table, mask, &mut idx) & 7) as usize;
        n += 1;
        if p != e {
            break p;
        }
    };

    // 相手チーム生成
    let etsv = next_rand_tbl(table, mask, &mut idx) ^ next_rand_tbl(table, mask, &mut idx);
    n += 2;
    for slot in &TEAMS[e] {
        n += gen_slot_batch(table, keys, mask, &mut idx, idx_limit, slot, etsv);
    }
    // 自トレーナー名決定
    let name = next_rand_tbl(table, mask, &mut idx) % 3;
    n += 1;
    // 自チーム生成
    let ptsv = next_rand_tbl(table, mask, &mut idx) ^ next_rand_tbl(table, mask, &mut idx);
    n += 2;
    for slot in &TEAMS[p] {
        n += gen_slot_batch(table, keys, mask, &mut idx, idx_limit, slot, ptsv);
    }

    // コード化
    (name * 8 + p as u32, n)
}
#[inline(always)]
fn next_rand_tbl(table: &[u16], mask: usize, idx: &mut usize) -> u32 {
    *idx += 1;
    (unsafe { *table.get_unchecked(*idx & mask) }) as u32
}

const GEN_SLOT_BATCH_SIZE: usize = 32;

#[derive(Clone, Copy)]
struct KeyRange {
    cond_code_lo: CondCode,
    cond_code_hi: CondCode,
    // cond_codeをブロードキャストしたもの
    #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
    vlo: std::arch::x86_64::__m256i,
    #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
    vhi: std::arch::x86_64::__m256i,
}

impl KeyRange {
    #[inline(always)]
    fn new(slot: &Slot) -> Self {
        #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
        {
            use std::arch::x86_64::*;
            // キー値は0..=124なのでi8の符号付き比較で範囲判定できる
            Self {
                cond_code_lo: slot.cond_code_lo,
                cond_code_hi: slot.cond_code_hi,
                vlo: unsafe { _mm256_set1_epi8(slot.cond_code_lo as i8 - 1) },
                vhi: unsafe { _mm256_set1_epi8(slot.cond_code_hi as i8 + 1) },
            }
        }
        #[cfg(not(all(target_arch = "x86_64", target_feature = "avx2")))]
        {
            Self {
                cond_code_lo: slot.cond_code_lo,
                cond_code_hi: slot.cond_code_hi,
            }
        }
    }

    #[inline(always)]
    fn accepts(&self, key: CondCode) -> bool {
        self.cond_code_lo <= key && key <= self.cond_code_hi
    }

    /// `keys[s0..s0+GEN_SLOT_BATCH_SIZE]`に対して一括で`accepts`を判定し、
    /// `keys[i]`に対応する結果を第iビットに詰めたu64で返す。
    ///
    /// 事前条件: `s0 + GEN_SLOT_BATCH_SIZE <= keys.len()`
    #[inline(always)]
    fn accepts_batch(&self, keys: &[CondCode], s0: usize) -> u64 {
        const B: usize = GEN_SLOT_BATCH_SIZE;
        const { assert!(B == 32 || B == 64) }; // これ要るのかなぁ…？
        debug_assert!(s0 + B <= keys.len());

        #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
        {
            use std::arch::x86_64::*;
            unsafe {
                let k0 = _mm256_loadu_si256(keys.as_ptr().add(s0) as *const __m256i);
                let a0 = _mm256_and_si256(
                    _mm256_cmpgt_epi8(k0, self.vlo),
                    _mm256_cmpgt_epi8(self.vhi, k0),
                );
                let m0 = _mm256_movemask_epi8(a0) as u32 as u64;
                if B == 64 {
                    let k1 = _mm256_loadu_si256(keys.as_ptr().add(s0 + 32) as *const __m256i);
                    let a1 = _mm256_and_si256(
                        _mm256_cmpgt_epi8(k1, self.vlo),
                        _mm256_cmpgt_epi8(self.vhi, k1),
                    );
                    let m1 = _mm256_movemask_epi8(a1) as u32 as u64;
                    m0 | (m1 << 32)
                } else {
                    m0
                }
            }
        }
        #[cfg(not(all(target_arch = "x86_64", target_feature = "avx2")))]
        {
            let win = unsafe { keys.get_unchecked(s0..s0 + B) };
            let mut hits: u64 = 0;
            for j in 0..B {
                let key = unsafe { *win.get_unchecked(j) };
                hits |= ((self.accepts(key)) as u64) << j;
            }
            hits
        }
    }
}

#[inline(always)]
fn gen_slot_batch(
    table: &[u16],
    // MEMO: 性格値再計算は1回に2消費ずつ進むので、個体の候補は1つおきに並ぶ。
    // そのため、奇数キーと偶数キーの2列に分けて取り回している。
    keys: [&[CondCode]; 2],
    mask: usize,
    idx: &mut usize,
    // NOTE: tableは他のスレッドと共有しており、並列で書き込みが行われるため、安全に読み出せる位置に限界がある
    idx_limit: usize,
    slot: &Slot,
    tsv: u32,
) -> u32 {
    *idx += 5;
    let mut n = 5u32;
    const B: usize = GEN_SLOT_BATCH_SIZE;

    let r2_mask = mask >> 1;
    let kp = keys[(*idx + 1) & 1];
    // MEMO: AVX2版において、ブロードキャスト済みの定数がループ中ymmレジスタに保持されることは確認済み
    let range = KeyRange::new(slot);

    const BATCH_SIZE: usize = 64;
    loop {
        let base = *idx;
        assert!(base + 2 <= idx_limit, "team generation exceeded lead");

        let q0 = base + 1; // 候補0のHIDを生成する乱数値の論理位置
        let s0 = (q0 >> 1) & r2_mask; // q0をkp内のindexへ変換したもの

        // リングバッファの回り込みなしにBATCH_SIZE個の候補を取ることができ、なおかつidx_limitにかかる危険もない場合
        // BATCH_SIZE個の候補を一括評価して高速に処理できるのでうれしい
        if s0 + BATCH_SIZE <= r2_mask + 1 && base + 2 * BATCH_SIZE <= idx_limit {
            const { assert!(BATCH_SIZE == 64) };
            let res_first = range.accepts_batch(kp, s0);
            let res_second = range.accepts_batch(kp, s0 + 32);
            let batch_result = res_first | (res_second << 32);
            if batch_result == 0 {
                // NOTE: 条件を満たす候補が見つかるまでに探索される候補数の平均（おおよそ50個 ※）より大きくBATCH_SIZEを取ることで、
                // ここの分岐に入る確率が小さくなり、CPUの分岐予測ミスが減らせる
                // (※ 性格は25通り、性別は性別比とかもあるけど大雑把に1/2と仮定)
                // 多少多めに判定することになってもSIMD化の恩恵のほうが大きい
                *idx += 2 * BATCH_SIZE;
                n += 2 * BATCH_SIZE as u32;
                continue;
            }

            // 下のbitから順に詰めてあるので、『accepts判定を通る候補で最も近いもの』はtrailing_zeros()で取得できる
            let hit_idx = batch_result.trailing_zeros() as usize;

            let hid = unsafe { *table.get_unchecked((q0 + 2 * hit_idx) & mask) } as u32;
            let lid = unsafe { *table.get_unchecked((q0 + 2 * hit_idx + 1) & mask) } as u32;
            *idx += 2 * (hit_idx + 1);
            n += 2 * (hit_idx as u32 + 1);
            // KeyRangeによる判定は色回避の発生を考慮していないため、別途チェックが必要
            if (hid ^ lid ^ tsv) >= 8 {
                // NOTE: 色回避が発生する確率は1/8192なので、この分岐は非常に高確率で通る
                return n;
            }

            // 色回避が発生した場合は、hit_idxの次の候補から検索が継続される
            continue;
        }

        // リングバッファの回り込みなしにGEN_SLOT_BATCH_SIZE個の候補を取ることができる場合
        if s0 + B <= r2_mask + 1 {
            let batch_result = range.accepts_batch(kp, s0);
            // idx_limitにかかる危険がない場合
            // なんかが全部信頼できるらしい
            if base + 2 * B <= idx_limit {
                let mut hits = batch_result;
                while hits != 0 {
                    let hit_idx = hits.trailing_zeros() as usize;

                    let hid = unsafe { *table.get_unchecked((q0 + 2 * hit_idx) & mask) } as u32;
                    let lid = unsafe { *table.get_unchecked((q0 + 2 * hit_idx + 1) & mask) } as u32;
                    if (hid ^ lid ^ tsv) >= 8 {
                        *idx += 2 * (hit_idx + 1);
                        return n + 2 * (hit_idx as u32 + 1);
                    }

                    hits &= hits - 1;
                }

                *idx += 2 * B;
                n += 2 * B as u32;
            }
            // idx_limitにかかる場合
            // 値を安全に読み出せる位置までで止める
            else {
                let valid = (idx_limit - base) / 2;
                debug_assert!(1 <= valid && valid < B);

                let mut batch_result = batch_result & ((1u64 << valid) - 1);
                while batch_result != 0 {
                    let hit_idx = batch_result.trailing_zeros() as usize;

                    let hid = unsafe { *table.get_unchecked((q0 + 2 * hit_idx) & mask) } as u32;
                    let lid = unsafe { *table.get_unchecked((q0 + 2 * hit_idx + 1) & mask) } as u32;
                    if (hid ^ lid ^ tsv) >= 8 {
                        *idx += 2 * (hit_idx + 1);
                        return n + 2 * (hit_idx as u32 + 1);
                    }

                    batch_result &= batch_result - 1;
                }

                panic!("team generation exceeded lead");
            }
        }
        // GEN_SLOT_BATCH_SIZE個の候補を取ろうとするとリングバッファの端をまたぐ場合
        // 仕方がないので、端を過ぎるまで1候補ずつ処理する
        else {
            // NOTE: s0はkp内のindexであることが保証されている
            let key = unsafe { *kp.get_unchecked(s0) };
            *idx += 2;
            n += 2;
            if range.accepts(key) {
                let hid = unsafe { *table.get_unchecked(q0 & mask) } as u32;
                let lid = unsafe { *table.get_unchecked((q0 + 1) & mask) } as u32;
                if (hid ^ lid ^ tsv) >= 8 {
                    return n;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::teamgen::generate_team;

    /// テーブル駆動生成(generate_team_from_table)がスカラー生成(generate_team)と
    /// 厳密に一致することを確認する(最適化経路の正しさの検証。最重要テスト)。
    /// キー列(make_cond_code)の正しさもここで同時に検証される(キーの符号化が受理条件と
    /// 等価でなければcode/n/最終状態のいずれかがずれる)。
    #[test]
    fn table_path_matches_scalar() {
        const LEN: usize = 1 << 15; // 32768
        const MASK: usize = LEN - 1;
        const N: u32 = 4096;

        for i in 0..N {
            let seed = i.wrapping_mul(0x9E3779B9);

            let mut states = vec![0u32; LEN];
            states[0] = seed;
            for j in 1..LEN {
                states[j] = crate::core::lcg::adv(states[j - 1]);
            }
            // テーブル経路は上位16bitとパリティ分割の事前計算キーを読む
            // (clight側のring.hi/ring.keypに相当)。
            let hi: Vec<u16> = states.iter().map(|&s| (s >> 16) as u16).collect();
            let mut keyp: [Vec<u8>; 2] = [vec![0u8; LEN / 2], vec![0u8; LEN / 2]];
            for q in 0..LEN {
                keyp[q & 1][q >> 1] = make_cond_code(hi[q], hi[(q + 1) & MASK]);
            }

            let mut s = seed;
            let c = generate_team(&mut s);
            let final_s = s;

            let (tc, tn) = generate_team_from_table(&hi, [&keyp[0], &keyp[1]], 0, MASK, LEN);
            assert_eq!(
                tc, c,
                "seed={:08X}: table/scalar code mismatch",
                seed
            );
            assert!(tn < LEN as u32, "seed={seed:08X}: count exceeds table");
            assert_eq!(
                states[tn as usize],
                final_s,
                "seed={:08X}: table/scalar final state mismatch",
                seed
            );
        }
    }

    #[test]
    #[should_panic(expected = "team generation exceeded lead")]
    fn insufficient_lead_panics() {
        let table = [0u16; 4];
        let keys = [[0u8; 2], [0u8; 2]];
        let mut idx = 0;
        gen_slot_batch(
            &table,
            [&keys[0], &keys[1]],
            3,
            &mut idx,
            1,
            &TEAMS[0][0],
            0,
        );
    }

    /// make_cond_codeのキー符号化がスロットの受理条件(性格・性別)と厳密に等価であることを、
    /// 全スロット定義 × 全性格 × 全性別バイトの網羅で確認する(色回避は対象外)。
    #[test]
    fn key_encoding_matches_accept_condition() {
        use crate::core::teamdef::{F, NG};
        for team in &TEAMS {
            for slot in team {
                let check_gender = slot.gender != NG;
                let want_female = slot.gender == F;
                // hi全域は広すぎるので、性格が全25値・性別バイトが全256値を通るように走査する
                for hi in (0..=u16::MAX).step_by(97) {
                    for lob in 0..=255u16 {
                        let lo = (hi.wrapping_mul(31)) & 0xFF00 | lob;
                        let pid = ((hi as u32) << 16) | lo as u32;
                        let g_ok =
                            !check_gender | ((((lo as u32) & 0xFF) < slot.ratio) == want_female);
                        let n_ok = pid % 25 == slot.nature as u32;
                        let key = make_cond_code(hi, lo);
                        assert_eq!(
                            g_ok & n_ok,
                            slot.cond_code_lo <= key && key <= slot.cond_code_hi,
                            "hi={:04X} lo={:04X} key={} range=[{},{}]",
                            hi,
                            lo,
                            key,
                            slot.cond_code_lo,
                            slot.cond_code_hi
                        );
                    }
                }
            }
        }
    }
}
