//! ポケモンコロシアムの「とにかくバトル」の「シングル・最強」のチーム生成処理

pub const A: u32 = 0x343FD;
pub const B: u32 = 0x269EC3;

#[inline(always)]
pub fn rand(s: &mut u32) -> u32 {
    *s = s.wrapping_mul(A).wrapping_add(B);
    *s >> 16
}

/// 固定枠の生成条件
#[derive(Clone, Copy)]
pub struct Slot {
    ratio: u32,
    gender: u8, // 0=性別不明, 1=♂, 2=♀
    nature: u32,
}

const NG: u8 = 0;
const M: u8 = 1;
const F: u8 = 2;

const fn sl(ratio: u32, gender: u8, nature: u32) -> Slot {
    Slot { ratio, gender, nature }
}

/// 性格
#[allow(dead_code)]
mod nature {
    pub const HARDY: u32 = 0;
    pub const LONELY: u32 = 1;
    pub const BRAVE: u32 = 2;
    pub const ADAMANT: u32 = 3;
    pub const NAUGHTY: u32 = 4;
    pub const BOLD: u32 = 5;
    pub const DOCILE: u32 = 6;
    pub const RELAXED: u32 = 7;
    pub const IMPISH: u32 = 8;
    pub const LAX: u32 = 9;
    pub const TIMID: u32 = 10;
    pub const HASTY: u32 = 11;
    pub const SERIOUS: u32 = 12;
    pub const JOLLY: u32 = 13;
    pub const NAIVE: u32 = 14;
    pub const MODEST: u32 = 15;
    pub const MILD: u32 = 16;
    pub const QUIET: u32 = 17;
    pub const BASHFUL: u32 = 18;
    pub const RASH: u32 = 19;
    pub const CALM: u32 = 20;
    pub const GENTLE: u32 = 21;
    pub const SASSY: u32 = 22;
    pub const CAREFUL: u32 = 23;
    pub const QUIRKY: u32 = 24;
}
use nature::*;

/// 性別比
/// 性別不明枠は慣例的に300(0x12C)が割り当てられる
const M7F1: u32 = 0x1F;
const M3F1: u32 = 0x3F;
const M1F1: u32 = 0x7F;
const M1F3: u32 = 0xBF;
const GENDERLESS: u32 = 0x12C;

#[rustfmt::skip]
pub const TEAMS: [[Slot; 6]; 8] = [
    // 0: バシャーモ/ラフレシア/ランターン/オニゴーリ/グランブル/ジュペッタ
    [sl(M7F1, M, SASSY), sl(M1F1, F, GENTLE), sl(M1F1, F, MODEST), sl(M1F1, M, RASH), sl(M1F3, M, NAUGHTY), sl(M1F1, F, NAUGHTY)],
    // 1: エンテイ/ゴローニャ/ベトベトン/コータス/ライボルト/ドククラゲ
    [sl(GENDERLESS, NG, HASTY), sl(M1F1, F, IMPISH), sl(M1F1, M, LONELY), sl(M1F1, M, MILD), sl(M1F1, F, MILD), sl(M1F1, M, SERIOUS)],
    // 2: ラグラージ/フーディン/ルンパッパ/トドゼルガ/ゴルダック/バクオング
    [sl(M7F1, M, BRAVE), sl(M3F1, F, MILD), sl(M1F1, M, MODEST), sl(M1F1, F, BASHFUL), sl(M1F1, M, MODEST), sl(M1F1, F, ADAMANT)],
    // 3: ライコウ/キュウコン/マタドガス/ツボツボ/アーマルド/ネイティオ
    [sl(GENDERLESS, NG, MILD), sl(M1F3, F, RASH), sl(M1F1, F, ADAMANT), sl(M1F1, F, SASSY), sl(M7F1, M, ADAMANT), sl(M1F1, M, QUIRKY)],
    // 4: メガニウム/バクフーン/オーダイル/エーフィ/ブラッキー/カイロス
    [sl(M7F1, M, QUIET), sl(M7F1, M, MILD), sl(M7F1, M, MODEST), sl(M7F1, M, RASH), sl(M7F1, M, BOLD), sl(M1F1, F, NAUGHTY)],
    // 5: スイクン/デンリュウ/ネンドール/オドシシ/ポリゴン2/ドンファン
    [sl(GENDERLESS, NG, MODEST), sl(M1F1, F, QUIET), sl(GENDERLESS, NG, LONELY), sl(M1F1, M, ADAMANT), sl(GENDERLESS, NG, RASH), sl(M1F1, F, ADAMANT)],
    // 6: メタグロス/ユレイドル/カイリキー/エアームド/サイドン/ハリテヤマ
    [sl(GENDERLESS, NG, LONELY), sl(M7F1, M, IMPISH), sl(M3F1, M, ADAMANT), sl(M1F1, F, LONELY), sl(M1F1, F, ADAMANT), sl(M3F1, M, ADAMANT)],
    // 7: ヘラクロス/ソーナンス/ミロカロス/ドードリオ/ノクタス/ヤミラミ
    [sl(M1F1, F, ADAMANT), sl(M1F1, M, TIMID), sl(M1F1, F, MODEST), sl(M1F1, M, ADAMANT), sl(M1F1, F, MODEST), sl(M1F1, M, ADAMANT)],
];

// LCG^5 の定数
// ダミーPID + 個体値 + 特性で5消費
const J5: (u32, u32) = (0x284A930D, 0xA2974C77);

/// LCGの1ステップ更新 s = s*A + B (randと異なり出力を捨てて状態だけ進める)。
#[inline(always)]
pub fn step(s: u32) -> u32 {
    s.wrapping_mul(A).wrapping_add(B)
}

/// LCGを任意ステップ数kだけ進めた状態を返す。
/// (乗数, 加数)の対を繰り返し二乗して合成する、O(log k)のジャンプ関数。
/// kはu32全域(2^32を法とするステップ数)を表せる。
pub fn lcg_jump(s: u32, mut k: u32) -> u32 {
    let mut acc_a: u32 = 1;
    let mut acc_b: u32 = 0;
    let mut base_a = A;
    let mut base_b = B;
    while k != 0 {
        if k & 1 != 0 {
            acc_b = base_a.wrapping_mul(acc_b).wrapping_add(base_b);
            acc_a = base_a.wrapping_mul(acc_a);
        }
        base_b = base_a.wrapping_mul(base_b).wrapping_add(base_b);
        base_a = base_a.wrapping_mul(base_a);
        k >>= 1;
    }
    acc_a.wrapping_mul(s).wrapping_add(acc_b)
}

/// チーム生成が消費する乱数の供給源。
/// next16()は「LCGを1ステップ進めて上位16bitを返す」、skip5()は
/// 「出力を捨てて5ステップ進める」(ダミーPID+個体値+特性の5消費)に対応する。
pub trait RandSrc {
    fn next16(&mut self) -> u32;
    fn skip5(&mut self);
}

/// LCG状態を直接進めるスカラー実装(従来と同一の計算)。
struct LcgSrc {
    s: u32,
}

impl RandSrc for LcgSrc {
    #[inline(always)]
    fn next16(&mut self) -> u32 {
        rand(&mut self.s)
    }
    #[inline(always)]
    fn skip5(&mut self) {
        self.s = self.s.wrapping_mul(J5.0).wrapping_add(J5.1);
    }
}

/// テーブル経路のバッチ再抽選候補数(調整対象。8/16/32で比較する)。
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
            // 連続ウィンドウ(ラップなし)。win[2j]がhiの取得元、win[2j+1]がloの取得元
            let win = unsafe { table.get_unchecked(start..start + 2 * B) };
            let mut hits: u32 = 0;
            for j in 0..B {
                let hi = unsafe { *win.get_unchecked(2 * j) } as u32;
                let lo = unsafe { *win.get_unchecked(2 * j + 1) } as u32;
                let pid = (hi << 16) | lo;
                let g_ok = !check_gender | (((lo & 0xFF) < slot.ratio) == want_female);
                let n_ok = pid % 25 == slot.nature;
                let s_ok = (hi ^ lo ^ tsv) >= 8;
                hits |= ((g_ok & n_ok & s_ok) as u32) << j;
            }
            // 書き込み済み範囲で完全に読めるレーン数だけを信用する(それ以降は古い値かもしれない)
            let valid = ((idx_limit - base) / 2).min(B);
            let valid_mask = if valid >= 32 { u32::MAX } else { (1u32 << valid) - 1 };
            let vhits = hits & valid_mask;
            if vhits != 0 {
                let j = vhits.trailing_zeros() as usize; // 書き込み済み範囲で最初の受理候補
                *idx += 2 * (j + 1);
                return Some(n + 2 * (j as u32 + 1));
            }
            if valid < B {
                // このバッチで書き込み位置に到達したが受理なし: スカラー再計算へ
                return None;
            }
            *idx += 2 * B;
            n += 2 * B as u32;
        } else {
            // 境界跨ぎ: 1候補ずつスカラーで(ラップを&maskで処理)
            let hi = unsafe { *table.get_unchecked((base + 1) & mask) } as u32;
            let lo = unsafe { *table.get_unchecked((base + 2) & mask) } as u32;
            *idx += 2;
            n += 2;
            let pid = (hi << 16) | lo;
            let g_ok = !check_gender | (((lo & 0xFF) < slot.ratio) == want_female);
            let n_ok = pid % 25 == slot.nature;
            let s_ok = (hi ^ lo ^ tsv) >= 8;
            if g_ok & n_ok & s_ok {
                return Some(n);
            }
        }
    }
}

/// ポケモン1匹の生成処理
/// 上記の5消費+条件込みPID決定。戻り値は消費したrand呼び出し回数(J5の5消費を含む)。
#[inline(always)]
fn gen_slot<R: RandSrc>(r: &mut R, slot: &Slot, tsv: u32) -> u32 {
    r.skip5();
    let mut n = 5u32;
    let check_gender = slot.gender != NG;
    let want_female = slot.gender == F;
    loop {
        let hi = r.next16();
        let lo = r.next16();
        n += 2;
        let pid = (hi << 16) | lo;

        // NOTE: CPUの投機的実行における分岐予測器に対する最適化で、ifを1つにまとめている。
        // たとえば性別比1:1のポケモンに対する性別判定は、
        // ループごとにifを通るか通らないかが半々のランダムであるため、
        // どれだけ予測をしても精度が50%より上がらない。
        // 一方で g_ok & n_ok & s_ok にまとめれば、ほぼ毎回falseになるため、
        // 分岐予測器の予測が当たりやすく、投機的実行のリターンが大きい。
        let g_ok = !check_gender | (((lo & 0xFF) < slot.ratio) == want_female);
        let n_ok = pid % 25 == slot.nature;
        let s_ok = (hi ^ lo ^ tsv) >= 8; // 色回避
        if g_ok & n_ok & s_ok {
            return n;
        }
    }
}

/// 1回分のチーム生成の内部実装。戻り値は (コード, 消費したrand呼び出し回数)。
/// generate_team / generate_team_with_count はこれを呼ぶだけの薄いラッパ。
#[inline(always)]
fn generate_team_impl<R: RandSrc>(r: &mut R) -> (u32, u32) {
    let mut n = 0u32;
    // 相手チーム決定
    let e = (r.next16() & 7) as usize;
    n += 1;
    // 自チーム決定
    let p = loop {
        let p = (r.next16() & 7) as usize;
        n += 1;
        if p != e {
            break p;
        }
    };

    // 相手チーム生成
    let etsv = r.next16() ^ r.next16();
    n += 2;
    for slot in &TEAMS[e] {
        n += gen_slot(r, slot, etsv);
    }
    // 自トレーナー名決定
    let name = r.next16() % 3;
    n += 1;
    // 自チーム生成
    let ptsv = r.next16() ^ r.next16();
    n += 2;
    for slot in &TEAMS[p] {
        n += gen_slot(r, slot, ptsv);
    }

    // コード化
    (name * 8 + p as u32, n)
}

/// 1回分のチーム生成
#[inline(always)]
pub fn generate_team(s: &mut u32) -> u32 {
    generate_team_with_count(s).0
}

/// 1回分のチーム生成。戻り値は (コード, 消費したrand呼び出し回数)。
/// LCG軌道順走査でのnの記録に用いる。
#[inline(always)]
pub fn generate_team_with_count(s: &mut u32) -> (u32, u32) {
    let mut src = LcgSrc { s: *s };
    let ret = generate_team_impl(&mut src);
    *s = src.s;
    ret
}

/// generate_team_from_table専用のテーブル読み。next_tab(table, mask, &mut idx)は
/// TableSrc::next16と同一の意味(idxを1進めてtable[idx&mask]の乱数値(上位16bit)を返す)。
/// tableは各要素が既に状態の上位16bitである`&[u16]`(clight側のring.hi)。
#[inline(always)]
fn next_tab(table: &[u16], mask: usize, idx: &mut usize) -> u32 {
    *idx += 1;
    (unsafe { *table.get_unchecked(*idx & mask) }) as u32
}

/// 事前計算済みの乱数値列(リングバッファ)から乱数値を読んで1回分のチーム生成を行う。
/// tableは各要素が状態の上位16bit(=その位置での乱数出力)である`&[u16]`。
/// table[(start_idx + i) & mask] が起点のiステップ先の乱数値であること、および
/// 呼び出し側の書き込み位置が論理位置`start_idx + lead`まで進んでいることを前提とする。
/// 戻り値は`Some((コード, 消費したrand呼び出し回数))`。消費数nがleadを超える(=書き込み位置を
/// 追い越して古いデータを読むことになる)場合は`None`を返すので、呼び出し側はスカラー
/// (`generate_team_with_count`)で再計算すること。
///
/// gen_slot_tableによるバッチ再抽選のみ汎用実装(generate_team_impl/gen_slot)と異なる。
/// e/p選択・etsv・name・ptsvの読み順と消費数nの数え方はgenerate_team_implと完全に同一にすること
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

/// 処理としては generate_team と同じ。
/// 生成されるコードが渡されたコードと一致するかを判定する。
/// early returnによる枝刈りが入っている。
#[inline(always)]
pub fn generate_team_checked(s: &mut u32, code: u32) -> bool {
    let mut src = LcgSrc { s: *s };
    let ok = generate_team_checked_impl(&mut src, code);
    *s = src.s;
    ok
}

#[inline(always)]
fn generate_team_checked_impl<R: RandSrc>(r: &mut R, code: u32) -> bool {
    let want_name = code / 8;
    let want_team = (code % 8) as usize;
    let e = (r.next16() & 7) as usize;
    let p = loop {
        let p = (r.next16() & 7) as usize;
        if p != e {
            break p;
        }
    };
    if p != want_team {
        return false;
    }
    let etsv = r.next16() ^ r.next16();
    for slot in &TEAMS[e] {
        gen_slot(r, slot, etsv);
    }
    let name = r.next16() % 3;
    if name != want_name {
        return false;
    }
    let ptsv = r.next16() ^ r.next16();
    for slot in &TEAMS[p] {
        gen_slot(r, slot, ptsv);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// selftest(main.rsのselftest関数)と同じ手順で(seed, code, 生成後seed)を作る。
    fn selftest_triples(n: u32) -> Vec<(u32, u32, u32)> {
        (0..n)
            .map(|i| {
                let seed = i.wrapping_mul(0x9E3779B9);
                let mut s = seed;
                let code = generate_team(&mut s);
                (seed, code, s)
            })
            .collect()
    }

    /// FNV-1a 64bit(teamgen本体には無いのでテスト内に小さく実装)。
    fn fnv1a64(bytes: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        h
    }

    /// selftestと同じ手順で作った(seed, code, 生成後seed)の並びに対する回帰チェック。
    /// このgoldenはC#参照実装(PokemonCoRNGLibrary)と256件一致確認済みの現行挙動
    /// (`codb-gen selftest 256`の出力)を固定したもの。teamgenのロジックに意図しない
    /// 変更が入った場合、以下のspot-checkまたはハッシュのいずれかが変化して回帰を検出する。
    #[test]
    fn core_golden() {
        const N: u32 = 4096;
        let triples = selftest_triples(N);

        // spot-check: 実際に生成して確認した先頭3件の(seed, code, 生成後seed)
        assert_eq!(triples[0], (0x0000_0000, 7, 0xFC91_E2D5));
        assert_eq!(triples[1], (0x9E37_79B9, 9, 0x65AB_3EA2));
        assert_eq!(triples[2], (0x3C6E_F372, 4, 0x3119_678B));

        let mut bytes = Vec::with_capacity(triples.len() * 12);
        for &(seed, code, s) in &triples {
            bytes.extend_from_slice(&seed.to_le_bytes());
            bytes.extend_from_slice(&code.to_le_bytes());
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        let hash = fnv1a64(&bytes);
        assert_eq!(hash, 0x41DE_69D3_5AA3_8328, "core_golden hash mismatch (regression in teamgen generation logic)");
    }

    /// generate_team_checkedが「スカラー生成と同じcodeならtrueかつ最終seedも一致」
    /// 「異なるcodeならfalse」を返すことを、多数のseedについて確認する
    /// (early return枝刈りの正しさの検証)。
    #[test]
    fn generate_team_checked_consistency() {
        const N: u32 = 4096;
        for i in 0..N {
            let seed = i.wrapping_mul(0x9E3779B9);

            let mut s1 = seed;
            let code = generate_team(&mut s1);

            let mut s2 = seed;
            assert!(generate_team_checked(&mut s2, code));
            assert_eq!(s2, s1);

            let mut s3 = seed;
            assert!(!generate_team_checked(&mut s3, (code + 1) % 24));
        }
    }

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
                states[j] = step(states[j - 1]);
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
