//! ポケモンコロシアムの「とにかくバトル」の「シングル・最強」のチーム生成処理
//! こっちはいろいろ最適化が施されている

use crate::teamdef::{Slot, F, NG, TEAMS};

// FIXME: 全体的にコメントがやかましい

/// 事前計算済みの乱数値列(リングバッファ)から乱数値を読んで1回分のチーム生成を行う。
/// tableは各要素が状態の上位16bit(=その位置での乱数出力)である`&[u16]`。
/// table[(start_idx + i) & mask] が起点のiステップ先の乱数値であること、および
/// 呼び出し側の書き込み位置が論理位置`start_idx + lead`まで進んでいることを前提とする。
/// 戻り値は`Some((コード, 消費したrand呼び出し回数))`。消費数nがleadを超える(=書き込み位置を
/// 追い越して古いデータを読むことになる)場合は`None`を返すので、呼び出し側はスカラー
/// (`teamgen::generate_team_with_count`)で再計算すること。
///
/// gen_slot_tableによるバッチ再抽選のみteamgen::generate_team_with_count/adv_gen_slotと異なる。
/// e/p選択・etsv・name・ptsvの読み順と消費数nの数え方はgenerate_team_with_countと完全に同一にすること
/// (これらの読み出しはidxが数十しか進まないため、lead>=2048程度であれば書き込み位置越えは
/// 事実上発生せず、チェックを省略している)。
#[inline(always)]
pub fn generate_team_from_table(table: &[u16], start_idx: usize, mask: usize, lead: usize) -> Option<(u32, u32)> {
    debug_assert!(mask < table.len());
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
        n += gen_slot_table(table, mask, &mut idx, idx_limit, slot, etsv)?;
    }
    // 自トレーナー名決定
    let name = next_tab(table, mask, &mut idx) % 3;
    n += 1;
    // 自チーム生成
    let ptsv = next_tab(table, mask, &mut idx) ^ next_tab(table, mask, &mut idx);
    n += 2;
    for slot in &TEAMS[p] {
        n += gen_slot_table(table, mask, &mut idx, idx_limit, slot, ptsv)?;
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

const GEN_SLOT_TABLE_BATCH: usize = 16;

/// テーブル経路専用のスロット再抽選。汎用gen_slotと同一の受理条件を、
/// 連続スライス上のバッチ走査で評価する(自動ベクトル化とブランチ削減狙い)。
/// idxは読み進めた位置を反映して更新する。戻り値はSome(消費rand数。skip5の5を含む)。
///
/// `idx_limit`は「読んでよい論理位置の上限」(呼び出し側の書き込み位置)。読み位置が
/// これを超える場合、書き込み位置を追い越して古いデータを読むことになるため`None`を返し、
/// 呼び出し側にスカラー再計算へフォールバックさせる。書き込み済み範囲内で受理が確定した場合の
/// 挙動(受理順位・n・idxの最終値)は先行書き込みが無い場合と完全に同一。
#[inline(always)]
fn gen_slot_table(table: &[u16], mask: usize, idx: &mut usize, idx_limit: usize, slot: &Slot, tsv: u32) -> Option<u32> {
    *idx += 5;
    let mut n = 5u32;
    let check_gender = slot.gender != NG;
    let want_female = slot.gender == F;
    const B: usize = GEN_SLOT_TABLE_BATCH;
    loop {
        let base = *idx;
        if base + 2 > idx_limit {
            // 最初の候補(base+1, base+2)すら書き込み済み範囲に収まらない
            return None;
        }
        let start = (base + 1) & mask;
        if start + 2 * B <= mask + 1 {
            // 連続ウィンドウ(ラップなし)。候補jはu16ペア(start+2j, start+2j+1)=(hi,lo)。
            // 受理判定をB=16候補ぶんまとめてビットマスクhitsに畳む。
            // AVX2版はvmovmskps畳み込みで直列レイテンシを削る(accept_mask8参照)。
            #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
            let hits: u32 = {
                debug_assert!(B == 16);
                let p = unsafe { table.as_ptr().add(start) };
                let m0 = unsafe { accept_mask8(p, tsv, slot.ratio, slot.nature as u32, check_gender, want_female) };
                let m1 = unsafe { accept_mask8(p.add(16), tsv, slot.ratio, slot.nature as u32, check_gender, want_female) };
                m0 | (m1 << 8)
            };
            #[cfg(not(all(target_arch = "x86_64", target_feature = "avx2")))]
            let hits: u32 = {
                let win = unsafe { table.get_unchecked(start..start + 2 * B) };
                let mut hits: u32 = 0;
                for j in 0..B {
                    let hi = unsafe { *win.get_unchecked(2 * j) } as u32;
                    let lo = unsafe { *win.get_unchecked(2 * j + 1) } as u32;
                    let pid = (hi << 16) | lo;
                    let g_ok = !check_gender | (((lo & 0xFF) < slot.ratio) == want_female);
                    let n_ok = pid % 25 == slot.nature as u32;
                    let s_ok = (hi ^ lo ^ tsv) >= 8;
                    hits |= ((g_ok & n_ok & s_ok) as u32) << j;
                }
                hits
            };
            // 高速パス: バッチ全体(base+1..base+2B)が書き込み済み範囲に収まるなら、
            // 全レーンが信用でき境界マスクは不要。fallback率0%の設定ではこちらがほぼ常に通り、
            // 減算・除算・マスク生成をクリティカルパス(バッチ間の直列レイテンシ鎖)から外す。
            if base + 2 * B <= idx_limit {
                if hits != 0 {
                    let j = hits.trailing_zeros() as usize;
                    *idx += 2 * (j + 1);
                    return Some(n + 2 * (j as u32 + 1));
                }
                *idx += 2 * B;
                n += 2 * B as u32;
            } else {
                // 書き込み位置近傍: 完全に読めるレーン数だけを信用する(それ以降は古い値かもしれない)
                let valid = ((idx_limit - base) / 2).min(B);
                let valid_mask = if valid >= 32 { u32::MAX } else { (1u32 << valid) - 1 };
                let vhits = hits & valid_mask;
                if vhits != 0 {
                    let j = vhits.trailing_zeros() as usize; // 書き込み済み範囲で最初の受理候補
                    *idx += 2 * (j + 1);
                    return Some(n + 2 * (j as u32 + 1));
                }
                // このバッチで書き込み位置に到達したが受理なし: スカラー再計算へ
                return None;
            }
        } else {
            // 境界跨ぎ: 1候補ずつスカラーで(ラップを&maskで処理)
            let hi = unsafe { *table.get_unchecked((base + 1) & mask) } as u32;
            let lo = unsafe { *table.get_unchecked((base + 2) & mask) } as u32;
            *idx += 2;
            n += 2;
            let pid = (hi << 16) | lo;
            let g_ok = !check_gender | (((lo & 0xFF) < slot.ratio) == want_female);
            let n_ok = pid % 25 == slot.nature as u32;
            let s_ok = (hi ^ lo ^ tsv) >= 8;
            if g_ok & n_ok & s_ok {
                return Some(n);
            }
        }
    }
}

/// AVX2で8候補の受理判定を行い、8bitマスク(bit i = 候補iが受理なら1)を返す。
/// `base`は連続する16個のu16(=8個のu32ペア(hi,lo))の先頭。候補jはu32レーンw_j
/// (リトルエンディアンで w = hi | (lo<<16))を使い、`pid = (hi<<16)|lo = rol(w,16)`。
/// 受理条件はスカラーの`gen_slot`/自動ベクトル版と厳密に同一(pid%25はLLVMと同じ
/// magic 0x51EB851F・>>35の逆数乗算で実装)。
///
/// LLVMの自動ベクトル化はbool→ビットマスクの畳み込みを約12命令の直列鎖(per-lane
/// ビット重み+水平OR)で出すが、ここでは`vmovmskps`1命令に置き換える。畳み込みが
/// 再抽選ループのクリティカルパス(次バッチへ進む分岐直前)にあるため効く。
///
/// 安全性: `base..base+16`(u16)が読み出し可能であること(呼び出し側の
/// `start + 2*B <= mask+1`ガードが保証)。avx2はtarget-cpu=nativeで有効。
#[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
#[inline(always)]
unsafe fn accept_mask8(base: *const u16, tsv: u32, ratio: u32, nature: u32, check_gender: bool, want_female: bool) -> u32 {
    use std::arch::x86_64::*;
    // 8個のu32レーン w = hi | (lo<<16) をロード
    let w = _mm256_loadu_si256(base as *const __m256i);
    let lo = _mm256_srli_epi32(w, 16);
    let hi = _mm256_and_si256(w, _mm256_set1_epi32(0xFFFF));
    let pid = _mm256_or_si256(_mm256_slli_epi32(hi, 16), lo);
    // pid % 25 = pid - (mulhi(pid, magic) >> 3) * 25   (magic=ceil(2^35/25)=0x51EB851F)
    let magic = _mm256_set1_epi32(0x51EB851Fu32 as i32);
    let prod_e = _mm256_mul_epu32(pid, magic); // 偶レーン(0,2,4,6)の64bit積
    let prod_o = _mm256_mul_epu32(_mm256_srli_epi64(pid, 32), magic); // 奇レーン(1,3,5,7)
    // 各積の上位dword(=mulhi)を集める: prod_eの上位は偶dword位置へ、prod_oの上位は奇dword位置へ
    let mulhi = _mm256_or_si256(
        _mm256_srli_epi64(prod_e, 32),
        _mm256_slli_epi64(_mm256_srli_epi64(prod_o, 32), 32),
    );
    let q = _mm256_srli_epi32(mulhi, 3);
    let r = _mm256_sub_epi32(pid, _mm256_mullo_epi32(q, _mm256_set1_epi32(25)));
    let n_ok = _mm256_cmpeq_epi32(r, _mm256_set1_epi32(nature as i32));
    // 色回避: (hi ^ lo ^ tsv) >= 8。値は<2^16なので符号付きcmpgt(x,7)で可。
    let x = _mm256_xor_si256(_mm256_xor_si256(hi, lo), _mm256_set1_epi32(tsv as i32));
    let s_ok = _mm256_cmpgt_epi32(x, _mm256_set1_epi32(7));
    let mut accept = _mm256_and_si256(n_ok, s_ok);
    if check_gender {
        // ((lo & 0xFF) < ratio) == want_female。lo&0xFF<256, ratio<256なので符号付きcmpgtで可。
        let lob = _mm256_and_si256(lo, _mm256_set1_epi32(0xFF));
        let g_lt = _mm256_cmpgt_epi32(_mm256_set1_epi32(ratio as i32), lob); // ratio > lob ⇔ lob < ratio
        let g_ok = if want_female { g_lt } else { _mm256_andnot_si256(g_lt, _mm256_set1_epi32(-1)) };
        accept = _mm256_and_si256(accept, g_ok);
    }
    // 各レーンの符号bit(受理なら1)を8bitに畳む。bit i = レーンi。
    _mm256_movemask_ps(_mm256_castsi256_ps(accept)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teamgen::generate_team_with_count;

    /// テーブル駆動生成(generate_team_from_table)がスカラー生成(generate_team_with_count)と
    /// 厳密に一致することを確認する(最適化経路の正しさの検証。最重要テスト)。
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
            // テーブル経路は上位16bitのみを読む(clight側のring.hiに相当)。
            let hi: Vec<u16> = states.iter().map(|&s| (s >> 16) as u16).collect();

            let mut s = seed;
            let (c, n) = generate_team_with_count(&mut s);
            let final_s = s;

            match generate_team_from_table(&hi, 0, MASK, LEN) {
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
}
