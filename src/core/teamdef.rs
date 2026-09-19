//! ポケモンコロシアムの「とにかくバトル」の「シングル・最強」の固定チーム定義

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Gender {
    NG = 0, // 性別不明
    M = 1,  // ♂
    F = 2,  // ♀
}
pub use Gender::{F, M, NG};

pub const fn gender_bucket(v: u32) -> u8 {
    (v >= 0x1F) as u8 + (v >= 0x3F) as u8 + (v >= 0x7F) as u8 + (v >= 0xBF) as u8
}

// TODO: この最適化について資料に起こす
const fn cond_code(ratio: u32, gender: Gender, nature: Nature) -> (u8, u8) {
    let r = gender_bucket(ratio);
    let (g_lo, g_hi) = match gender {
        Gender::NG => (0, 4),
        Gender::F => (0, r - 1),
        Gender::M => (r, 4),
    };
    let base = (nature as u32 * 5) as u8;

    (base + g_lo, base + g_hi)
}

#[derive(Clone, Copy)]
pub struct Slot {
    pub ratio: u32,
    pub gender: Gender,
    pub nature: Nature,
    pub cond_code_lo: u8,
    pub cond_code_hi: u8,
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
#[allow(dead_code)]
pub enum Nature {
    HARDY = 0,
    LONELY = 1,
    BRAVE = 2,
    ADAMANT = 3,
    NAUGHTY = 4,
    BOLD = 5,
    DOCILE = 6,
    RELAXED = 7,
    IMPISH = 8,
    LAX = 9,
    TIMID = 10,
    HASTY = 11,
    SERIOUS = 12,
    JOLLY = 13,
    NAIVE = 14,
    MODEST = 15,
    MILD = 16,
    QUIET = 17,
    BASHFUL = 18,
    RASH = 19,
    CALM = 20,
    GENTLE = 21,
    SASSY = 22,
    CAREFUL = 23,
    QUIRKY = 24,
}
use Nature::*;

// 性別比
const M7F1: u32 = 0x1F;
const M3F1: u32 = 0x3F;
const M1F1: u32 = 0x7F;
const M1F3: u32 = 0xBF;
const GENDERLESS: u32 = 0x12C; // 性別不明枠は慣例的に300(0x12C)が割り当てられる

const fn sl(ratio: u32, gender: Gender, nature: Nature) -> Slot {
    let (cond_code_lo, cond_code_hi) = cond_code(ratio, gender, nature);
    Slot {
        ratio,
        gender,
        nature,
        cond_code_lo,
        cond_code_hi,
    }
}

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
