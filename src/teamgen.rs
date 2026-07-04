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

/// ポケモン1匹の生成処理
/// 上記の5消費＋条件込みPID決定
#[inline(always)]
fn gen_slot(s: &mut u32, slot: &Slot, tsv: u32) {
    *s = s.wrapping_mul(J5.0).wrapping_add(J5.1);
    let check_gender = slot.gender != NG;
    let want_female = slot.gender == F;
    loop {
        let hi = rand(s);
        let lo = rand(s);
        let pid = (hi << 16) | lo;
        
        // NOTE: CPUの投機的実行の予測機に対する最適化で、ifを1つにまとめている。
        // たとえば性別比1:1のポケモンに対する性別判定は、
        // ループごとにifを通るか通らないかが半々のランダムであるため、
        // どれだけ予測をしても精度が50%より上がらない。
        // 一方で g_ok & n_ok & s_ok にまとめれば、ほぼ毎回falseになるため、
        // 予測機の予測が当たりやすく、投機的実行のリターンが大きい
        let g_ok = !check_gender | (((lo & 0xFF) < slot.ratio) == want_female);
        let n_ok = pid % 25 == slot.nature;
        let s_ok = (hi ^ lo ^ tsv) >= 8; // 色回避
        if g_ok & n_ok & s_ok {
            return;
        }
    }
}

/// 1回分のチーム生成
#[inline(always)]
pub fn generate_team(s: &mut u32) -> u32 {
    // 相手チーム決定
    let e = (rand(s) & 7) as usize;
    // 自チーム決定
    let p = loop {
        let p = (rand(s) & 7) as usize;
        if p != e {
            break p;
        }
    };

    // 相手チーム生成
    let etsv = rand(s) ^ rand(s);
    for slot in &TEAMS[e] {
        gen_slot(s, slot, etsv);
    }
    // 自トレーナー名決定
    let name = rand(s) % 3;
    // 自チーム生成
    let ptsv = rand(s) ^ rand(s);
    for slot in &TEAMS[p] {
        gen_slot(s, slot, ptsv);
    }

    // コード化
    name * 8 + p as u32
}

/// 処理としては generate_team と同じ
/// 生成されるコードが渡されたコードと一致するかを判定する
/// early returnによる枝刈りが入っている
#[inline(always)]
pub fn generate_team_checked(s: &mut u32, code: u32) -> bool {
    let want_name = code / 8;
    let want_team = (code % 8) as usize;
    let e = (rand(s) & 7) as usize;
    let p = loop {
        let p = (rand(s) & 7) as usize;
        if p != e {
            break p;
        }
    };
    if p != want_team {
        return false;
    }
    let etsv = rand(s) ^ rand(s);
    for slot in &TEAMS[e] {
        gen_slot(s, slot, etsv);
    }
    let name = rand(s) % 3;
    if name != want_name {
        return false;
    }
    let ptsv = rand(s) ^ rand(s);
    for slot in &TEAMS[p] {
        gen_slot(s, slot, ptsv);
    }
    true
}
