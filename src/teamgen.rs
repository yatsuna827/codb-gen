//! ポケモンコロシアムの「とにかくバトル」の「シングル・最強」のチーム生成処理

use crate::lcg;
use crate::teamdef::{Slot, F, NG, TEAMS};

/// 1回分のチーム生成。戻り値はコード。
#[inline(always)]
pub fn generate_team(s: &mut u32) -> u32 {
    // 相手チーム決定
    let e = (lcg::rand(s) & 7) as usize;
    // 自チーム決定
    let p = loop {
        let p = (lcg::rand(s) & 7) as usize;
        if p != e {
            break p;
        }
    };

    // 相手チーム生成
    let etsv = lcg::rand(s) ^ lcg::rand(s);
    for slot in &TEAMS[e] {
        adv_gen_slot(s, slot, etsv);
    }
    // 自トレーナー名決定
    let name = lcg::rand(s) % 3;
    // 自チーム生成
    let ptsv = lcg::rand(s) ^ lcg::rand(s);
    for slot in &TEAMS[p] {
        adv_gen_slot(s, slot, ptsv);
    }

    // コード化
    name * 8 + p as u32
}

/// 1回分のチーム生成。戻り値は (コード, LCGの消費数)。
#[inline(always)]
pub fn generate_team_with_count(s: &mut u32) -> (u32, u32) {
    let mut n = 0u32;
    // 相手チーム決定
    let e = (lcg::rand(s) & 7) as usize;
    n += 1;
    // 自チーム決定
    let p = loop {
        let p = (lcg::rand(s) & 7) as usize;
        n += 1;
        if p != e {
            break p;
        }
    };

    // 相手チーム生成
    let etsv = lcg::rand(s) ^ lcg::rand(s);
    n += 2;
    for slot in &TEAMS[e] {
        n += adv_gen_slot(s, slot, etsv);
    }
    // 自トレーナー名決定
    let name = lcg::rand(s) % 3;
    n += 1;
    // 自チーム生成
    let ptsv = lcg::rand(s) ^ lcg::rand(s);
    n += 2;
    for slot in &TEAMS[p] {
        n += adv_gen_slot(s, slot, ptsv);
    }

    // コード化
    (name * 8 + p as u32, n)
}

/// 生成されるチームが`code`で指定されたものと一致するかを判定する。
/// early returnによる枝刈りが入っている。
#[inline(always)]
pub fn generate_team_checked(s: &mut u32, code: u32) -> bool {
    let want_name = code / 8;
    let want_team = (code % 8) as usize;
    let e = (lcg::rand(s) & 7) as usize;
    let p = loop {
        let p = (lcg::rand(s) & 7) as usize;
        if p != e {
            break p;
        }
    };
    if p != want_team {
        return false;
    }
    let etsv = lcg::rand(s) ^ lcg::rand(s);
    for slot in &TEAMS[e] {
        adv_gen_slot(s, slot, etsv);
    }
    let name = lcg::rand(s) % 3;
    if name != want_name {
        return false;
    }
    let ptsv = lcg::rand(s) ^ lcg::rand(s);
    for slot in &TEAMS[p] {
        adv_gen_slot(s, slot, ptsv);
    }
    true
}

/// ポケモン1匹の生成処理。戻り値はLCGの消費数。
#[inline(always)]
fn adv_gen_slot(s: &mut u32, slot: &Slot, tsv: u32) -> u32 {
    *s = lcg::adv5(*s);
    let mut n = 5u32;
    let check_gender = slot.gender != NG;
    let want_female = slot.gender == F;
    loop {
        let hi = lcg::rand(s);
        let lo = lcg::rand(s);
        n += 2;
        let pid = (hi << 16) | lo;

        // NOTE: CPUの投機的実行における分岐予測器に対する最適化で、ifを1つにまとめている。
        // たとえば性別比1:1のポケモンに対する性別判定は、
        // ループごとにifを通るか通らないかが半々のランダムであるため、
        // どれだけ予測をしても精度が50%より上がらない。
        // 一方で g_ok & n_ok & s_ok にまとめれば、ほぼ毎回falseになるため、
        // 分岐予測器の予測が当たりやすく、投機的実行のリターンが大きい。
        let g_ok = !check_gender | (((lo & 0xFF) < slot.ratio) == want_female);
        let n_ok = pid % 25 == slot.nature as u32;
        let s_ok = (hi ^ lo ^ tsv) >= 8; // 色回避
        if g_ok & n_ok & s_ok {
            return n;
        }
    }
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

}
