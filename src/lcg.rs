//! 純粋な32bit LCG(線形合同法)のロジック

pub const A: u32 = 0x343FD;
pub const B: u32 = 0x269EC3;

#[inline(always)]
pub fn rand(s: &mut u32) -> u32 {
    *s = s.wrapping_mul(A).wrapping_add(B);
    *s >> 16
}

/// LCGを1消費する。戻り値は更新後のseed。
#[inline(always)]
pub fn adv(s: u32) -> u32 {
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

// LCG^5 の定数
// ダミーPID + 個体値 + 特性で5消費
const J5: (u32, u32) = (0x284A930D, 0xA2974C77);

/// LCGを5消費する。戻り値は更新後のseed。
#[inline(always)]
pub fn adv5(s: u32) -> u32 {
    s.wrapping_mul(J5.0).wrapping_add(J5.1)
}
