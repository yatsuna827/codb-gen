//! ポケモンコロシアムの「とにかくバトル」の「シングル・最強」のチーム生成処理
//! こっちはいろいろ最適化が施されている

use crate::teamdef::{gender_bucket, Slot, TEAMS};

// FIXME: 全体的にコメントがやかましい

/// 位置qの事前計算キー。pid = (hi<<16)|lo(hi=位置qの乱数値、lo=位置q+1の乱数値)に対し
/// `性格(pid % 25) * 5 + 性別バケツ(gender_bucket(lo & 0xFF))` を返す(値域0..=124)。
/// スロットの受理条件のうち性格・性別は「key ∈ [slot.klo, slot.khi]」と厳密に等価
/// (teamdef::sl参照)。色回避(hi^lo^tsv >= 8)はキーに含まれないため、
/// キー一致した候補に対してのみ別途検査する。
///
/// fillが位置ごとに1回だけ計算して`ring.key`に書き、再抽選走査はこの1バイトの
/// 範囲比較だけで受理判定する(各位置のPIDは平均数百回、異なるスロットから
/// 繰り返し評価されるため、pid%25等の再計算を位置ごと1回に集約できる)。
#[inline(always)]
pub fn make_key(hi: u16, lo: u16) -> u8 {
    let pid = ((hi as u32) << 16) | lo as u32;
    let nature = (pid % 25) as u8;
    nature * 5 + gender_bucket((lo & 0xFF) as u32)
}

/// 事前計算済みの乱数値列(リングバッファ)から乱数値を読んで1回分のチーム生成を行う。
/// tableは各要素が状態の上位16bit(=その位置での乱数出力)である`&[u16]`、
/// keysはパリティ分割された事前計算キー列(`make_key`)で、位置qのキーは
/// `keys[q & 1][(q >> 1) & (mask >> 1)]`に格納されている(候補はストライド2で並ぶため、
/// パリティで分割すると1スロットの候補キー列が連続バイトになり、SIMDロードが密になる)。
/// table[(start_idx + i) & mask] が起点のiステップ先の乱数値であること、および
/// 呼び出し側の書き込み位置が論理位置`start_idx + lead`(キーは`start_idx + lead - 1`)まで
/// 進んでいることを前提とする。
/// 戻り値は`Some((コード, 消費したrand呼び出し回数))`。消費数nがleadを超える(=書き込み位置を
/// 追い越して古いデータを読むことになる)場合は`None`を返すので、呼び出し側はスカラー
/// (`teamgen::generate_team_with_count`)で再計算すること。
///
/// gen_slot_tableによるバッチ再抽選のみteamgen::generate_team_with_count/adv_gen_slotと異なる。
/// e/p選択・etsv・name・ptsvの読み順と消費数nの数え方はgenerate_team_with_countと完全に同一にすること
/// (これらの読み出しはidxが数十しか進まないため、lead>=2048程度であれば書き込み位置越えは
/// 事実上発生せず、チェックを省略している)。
#[inline(always)]
pub fn generate_team_from_table(
    table: &[u16],
    keys: [&[u8]; 2],
    start_idx: usize,
    mask: usize,
    lead: usize,
) -> Option<(u32, u32)> {
    debug_assert!(mask < table.len());
    debug_assert!(keys[0].len() == table.len() / 2 && keys[1].len() == table.len() / 2);
    let idx_limit = start_idx + lead;
    let mut idx = start_idx;
    let mut n = 0u32;

    // 相手チーム決定
    let e = (next_tab(table, mask, &mut idx) & 7) as usize;
    n += 1;
    // 自チーム決定
    let p = loop {
        let p = (next_tab(table, mask, &mut idx) & 7) as usize;
        n += 1;
        if p != e {
            break p;
        }
    };

    // 相手チーム生成
    let etsv = next_tab(table, mask, &mut idx) ^ next_tab(table, mask, &mut idx);
    n += 2;
    for slot in &TEAMS[e] {
        n += gen_slot_table(table, keys, mask, &mut idx, idx_limit, slot, etsv)?;
    }
    // 自トレーナー名決定
    let name = next_tab(table, mask, &mut idx) % 3;
    n += 1;
    // 自チーム生成
    let ptsv = next_tab(table, mask, &mut idx) ^ next_tab(table, mask, &mut idx);
    n += 2;
    for slot in &TEAMS[p] {
        n += gen_slot_table(table, keys, mask, &mut idx, idx_limit, slot, ptsv)?;
    }

    // コード化
    Some((name * 8 + p as u32, n))
}

/// generate_team_from_table専用のテーブル読み。next_tab(table, mask, &mut idx)は
/// idxを1進めてtable[idx&mask]の乱数値(上位16bit)を返す。
/// tableは各要素が既に状態の上位16bitである`&[u16]`(clight側のring.hi)。
#[inline(always)]
fn next_tab(table: &[u16], mask: usize, idx: &mut usize) -> u32 {
    *idx += 1;
    (unsafe { *table.get_unchecked(*idx & mask) }) as u32
}


const GEN_SLOT_TABLE_BATCH: usize = 32;

/// スロットの受理キー範囲[klo, khi]の比較用に事前展開した判定器。
/// AVX2版はブロードキャスト済みのymm定数を保持する(再抽選ループの毎バッチで
/// `vpbroadcastb`×2とSlotフィールドの再ロードがクリティカルパスに乗るのを防ぐため、
/// スロットごとに1回だけ構築して使い回す)。
#[derive(Clone, Copy)]
struct KeyRange {
    klo: u8,
    khi: u8,
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
            // キー値は0..=124なのでi8の符号付き比較で範囲判定できる(klo-1 >= -1, khi+1 <= 125)
            Self {
                klo: slot.klo,
                khi: slot.khi,
                vlo: unsafe { _mm256_set1_epi8(slot.klo as i8 - 1) },
                vhi: unsafe { _mm256_set1_epi8(slot.khi as i8 + 1) },
            }
        }
        #[cfg(not(all(target_arch = "x86_64", target_feature = "avx2")))]
        {
            Self { klo: slot.klo, khi: slot.khi }
        }
    }

    /// 1候補のキー範囲判定(スカラー経路用)。
    #[inline(always)]
    fn accepts(&self, key: u8) -> bool {
        self.klo <= key && key <= self.khi
    }

    /// パリティ分割キー列の連続するBバイトkp[s0..s0+B]に対する範囲比較の
    /// ヒットマスクを返す(bit j = 候補j)。
    ///
    /// 安全性: kp[s0..s0+B]が読める(呼び出し側の`s0 + B <= r2_mask + 1`ガードが保証)。
    #[inline(always)]
    fn batch_hits(&self, kp: &[u8], s0: usize) -> u64 {
        const B: usize = GEN_SLOT_TABLE_BATCH;
        const { assert!(B == 32 || B == 64) };
        debug_assert!(s0 + B <= kp.len());
        #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
        {
            use std::arch::x86_64::*;
            unsafe {
                let k0 = _mm256_loadu_si256(kp.as_ptr().add(s0) as *const __m256i);
                let a0 = _mm256_and_si256(_mm256_cmpgt_epi8(k0, self.vlo), _mm256_cmpgt_epi8(self.vhi, k0));
                let m0 = _mm256_movemask_epi8(a0) as u32 as u64;
                if B == 64 {
                    let k1 = _mm256_loadu_si256(kp.as_ptr().add(s0 + 32) as *const __m256i);
                    let a1 = _mm256_and_si256(_mm256_cmpgt_epi8(k1, self.vlo), _mm256_cmpgt_epi8(self.vhi, k1));
                    let m1 = _mm256_movemask_epi8(a1) as u32 as u64;
                    m0 | (m1 << 32)
                } else {
                    m0
                }
            }
        }
        #[cfg(not(all(target_arch = "x86_64", target_feature = "avx2")))]
        {
            let win = unsafe { kp.get_unchecked(s0..s0 + B) };
            let mut hits: u64 = 0;
            for j in 0..B {
                let key = unsafe { *win.get_unchecked(j) };
                hits |= ((self.accepts(key)) as u64) << j;
            }
            hits
        }
    }
}

/// テーブル経路専用のスロット再抽選。汎用gen_slotと同一の受理条件を、
/// 事前計算キー列の範囲比較によるバッチ走査で評価する。
/// idxは読み進めた位置を反映して更新する。戻り値はSome(消費rand数。skip5の5を含む)。
///
/// 受理条件の分解: 性格・性別はキー範囲比較(batch_key_hits)で、色回避(1/8192でしか
/// 落ちない)はキーヒットした候補に対するスカラー検査で行う。キーヒットの列を
/// 下位ビットから消費するため「最初に全条件を満たす候補」はスカラー版と厳密に一致する
/// (色回避で落ちた候補はhitsから外して次のヒットへ進む。nの数えは候補通過数×2で同一)。
///
/// `idx_limit`は「読んでよい論理位置の上限」(呼び出し側の書き込み位置)。読み位置が
/// これを超える場合、書き込み位置を追い越して古いデータを読むことになるため`None`を返し、
/// 呼び出し側にスカラー再計算へフォールバックさせる。書き込み済み範囲内で受理が確定した場合の
/// 挙動(受理順位・n・idxの最終値)は先行書き込みが無い場合と完全に同一。
#[inline(always)]
fn gen_slot_table(
    table: &[u16],
    keys: [&[u8]; 2],
    mask: usize,
    idx: &mut usize,
    idx_limit: usize,
    slot: &Slot,
    tsv: u32,
) -> Option<u32> {
    *idx += 5;
    let mut n = 5u32;
    const B: usize = GEN_SLOT_TABLE_BATCH;
    // このスロットの候補は位置base+1, base+3, ...とストライド2で並ぶため、
    // パリティ(base+1)&1は再抽選中一定(バッチ前進は+2B、スカラー前進は+2)。
    // 対応する分割キー列と範囲判定器をループ外で1回だけ構築する。
    let r2_mask = mask >> 1;
    let kp = keys[(*idx + 1) & 1];
    let range = KeyRange::new(slot);
    // 一括評価する候補数(W/Bバッチぶんの固定窓)。
    // 平均候補数(約50)より大きく取り、「窓内にヒット無し」で次の窓へ進む分岐を
    // 少数派(約26%)にして分岐予測ミスを減らす。ヒット位置の選択はu64のtzcntで分岐なし。
    const W: usize = 64;
    loop {
        let base = *idx;
        if base + 2 > idx_limit {
            // 最初の候補(base+1, base+2)すら書き込み済み範囲に収まらない
            return None;
        }
        let q0 = base + 1; // 候補0のhi位置(論理)
        let s0 = (q0 >> 1) & r2_mask; // 分割キー列内のインデックス

        // 超高速パス: 固定窓W候補(分割キー列でWバイト)がラップせず、かつ全候補が
        // 書き込み済み範囲に収まる場合、W候補を無条件に一括評価する。
        // 「最初のキーヒット」を分岐なしで選ぶため、スロットあたりの予測不能な分岐が
        // 「窓内ヒット無し」の1本に減る。評価候補の余剰(平均約50に対しW=64)は
        // SIMDのスループット余剰で吸収される。受理順位はバッチ逐次版と厳密に同一。
        if s0 + W <= r2_mask + 1 && base + 2 * W <= idx_limit {
            const { assert!(W == 64) };
            let m0 = range.batch_hits(kp, s0);
            let m1 = range.batch_hits(kp, s0 + 32);
            let full = m0 | (m1 << 32);
            if full == 0 {
                *idx += 2 * W;
                n += 2 * W as u32;
                continue;
            }
            let j = full.trailing_zeros() as usize; // 窓内最初のキーヒット
            // 色回避のみここで検査(キーは性格・性別のみを含む)。
            // hi/loの物理位置はリング折り返しがありうるため&maskで読む。
            let hi = unsafe { *table.get_unchecked((q0 + 2 * j) & mask) } as u32;
            let lo = unsafe { *table.get_unchecked((q0 + 2 * j + 1) & mask) } as u32;
            *idx += 2 * (j + 1);
            n += 2 * (j as u32 + 1);
            if (hi ^ lo ^ tsv) >= 8 {
                return Some(n);
            }
            // 色回避棄却(1/8192): 候補jまで消費済みなので、次の候補から仕切り直す
            continue;
        }
        if s0 + B <= r2_mask + 1 {
            // 分割キー列上で連続(ラップなし)。候補jのキーはkp[s0+j]。
            let m = range.batch_hits(kp, s0);
            // 高速パス: バッチ全体(候補B個=位置base+1..base+2B)が書き込み済み範囲に
            // 収まるなら、全レーンが信用でき境界マスクは不要。fallback率0%の設定では
            // こちらがほぼ常に通り、境界計算をクリティカルパスから外す。
            if base + 2 * B <= idx_limit {
                let mut hits = m;
                while hits != 0 {
                    let j = hits.trailing_zeros() as usize;
                    // 色回避のみここで検査(キーは性格・性別のみを含む)。
                    // hi/loの物理位置はリング折り返しがありうるため&maskで読む(稀な経路)。
                    let hi = unsafe { *table.get_unchecked((q0 + 2 * j) & mask) } as u32;
                    let lo = unsafe { *table.get_unchecked((q0 + 2 * j + 1) & mask) } as u32;
                    if (hi ^ lo ^ tsv) >= 8 {
                        *idx += 2 * (j + 1);
                        return Some(n + 2 * (j as u32 + 1));
                    }
                    hits &= hits - 1;
                }
                *idx += 2 * B;
                n += 2 * B as u32;
            } else {
                // 書き込み位置近傍: 完全に読めるレーン数だけを信用する(それ以降は古い値かもしれない)
                let valid = (idx_limit - base) / 2; // 1 <= valid < B がこの分岐で保証される
                debug_assert!(1 <= valid && valid < B);
                let mut vhits = m & ((1u64 << valid) - 1);
                while vhits != 0 {
                    let j = vhits.trailing_zeros() as usize; // 書き込み済み範囲で最初のキーヒット
                    let hi = unsafe { *table.get_unchecked((q0 + 2 * j) & mask) } as u32;
                    let lo = unsafe { *table.get_unchecked((q0 + 2 * j + 1) & mask) } as u32;
                    if (hi ^ lo ^ tsv) >= 8 {
                        *idx += 2 * (j + 1);
                        return Some(n + 2 * (j as u32 + 1));
                    }
                    vhits &= vhits - 1;
                }
                // このバッチで書き込み位置に到達したが受理なし: スカラー再計算へ
                return None;
            }
        } else {
            // 分割キー列の境界跨ぎ: 1候補ずつスカラーで。
            // 単一キーの読みkp[s0]はラップと無関係に常に正しい(fillが論理ストリームから計算済み)。
            let key = unsafe { *kp.get_unchecked(s0) };
            *idx += 2;
            n += 2;
            if range.accepts(key) {
                let hi = unsafe { *table.get_unchecked(q0 & mask) } as u32;
                let lo = unsafe { *table.get_unchecked((q0 + 1) & mask) } as u32;
                if (hi ^ lo ^ tsv) >= 8 {
                    return Some(n);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teamgen::generate_team_with_count;

    /// テーブル駆動生成(generate_team_from_table)がスカラー生成(generate_team_with_count)と
    /// 厳密に一致することを確認する(最適化経路の正しさの検証。最重要テスト)。
    /// キー列(make_key)の正しさもここで同時に検証される(キーの符号化が受理条件と
    /// 等価でなければcode/n/最終状態のいずれかがずれる)。
    #[test]
    fn table_path_matches_scalar() {
        const LEN: usize = 1 << 15; // 32768
        const MASK: usize = LEN - 1;
        const N: u32 = 4096;

        let mut skip_count = 0u32;
        for i in 0..N {
            let seed = i.wrapping_mul(0x9E3779B9);

            let mut states = vec![0u32; LEN];
            states[0] = seed;
            for j in 1..LEN {
                states[j] = crate::lcg::adv(states[j - 1]);
            }
            // テーブル経路は上位16bitとパリティ分割の事前計算キーを読む
            // (clight側のring.hi/ring.keypに相当)。
            let hi: Vec<u16> = states.iter().map(|&s| (s >> 16) as u16).collect();
            let mut keyp: [Vec<u8>; 2] = [vec![0u8; LEN / 2], vec![0u8; LEN / 2]];
            for q in 0..LEN {
                keyp[q & 1][q >> 1] = make_key(hi[q], hi[(q + 1) & MASK]);
            }

            let mut s = seed;
            let (c, n) = generate_team_with_count(&mut s);
            let final_s = s;

            match generate_team_from_table(&hi, [&keyp[0], &keyp[1]], 0, MASK, LEN) {
                None => {
                    // n >= LEN。事実上起きないはずだが、起きてもスキップして継続する。
                    skip_count += 1;
                }
                Some((tc, tn)) => {
                    assert_eq!((tc, tn), (c, n), "seed={:08X}: table/scalar code or count mismatch", seed);
                    assert_eq!(
                        states[n as usize & MASK],
                        final_s,
                        "seed={:08X}: table/scalar final state mismatch",
                        seed
                    );
                }
            }
        }

        assert!(
            (skip_count as f64) < (N as f64) * 0.01,
            "too many skips: {}/{} (>=1%)",
            skip_count,
            N
        );
    }

    /// make_keyのキー符号化がスロットの受理条件(性格・性別)と厳密に等価であることを、
    /// 全スロット定義 × 全性格 × 全性別バイトの網羅で確認する(色回避は対象外)。
    #[test]
    fn key_encoding_matches_accept_condition() {
        use crate::teamdef::{F, NG};
        for team in &TEAMS {
            for slot in team {
                let check_gender = slot.gender != NG;
                let want_female = slot.gender == F;
                // hi全域は広すぎるので、性格が全25値・性別バイトが全256値を通るように走査する
                for hi in (0..=u16::MAX).step_by(97) {
                    for lob in 0..=255u16 {
                        let lo = (hi.wrapping_mul(31)) & 0xFF00 | lob;
                        let pid = ((hi as u32) << 16) | lo as u32;
                        let g_ok = !check_gender | ((((lo as u32) & 0xFF) < slot.ratio) == want_female);
                        let n_ok = pid % 25 == slot.nature as u32;
                        let key = make_key(hi, lo);
                        assert_eq!(
                            g_ok & n_ok,
                            slot.klo <= key && key <= slot.khi,
                            "hi={:04X} lo={:04X} key={} range=[{},{}]",
                            hi,
                            lo,
                            key,
                            slot.klo,
                            slot.khi
                        );
                    }
                }
            }
        }
    }
}
