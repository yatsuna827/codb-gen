use super::rice_coding::RiceCodingWriter;

// NOTE:
// 論理的には、codbは(キー,seed上位16bit)のペア（『エントリ』）をキー昇順でソートしたリスト。
// ここで、キー昇順でソートされているため、「キーの値が`P`であるエントリの個数」を累積和で持っておくことにより、
// 『キーの値が`P`であるエントリのインデックスの範囲』には2回のアクセスで特定できる。
// つまり、時間効率を大きく落とすことなく空間効率を上げることができる。

pub(crate) fn encode_counts(counts: &[u32], k: u8) -> Vec<u8> {
    let mut writer = RiceCodingWriter::new();
    for &count in counts {
        writer.write(encode_count(count), k);
    }
    writer.build()
}

/// Rice符号化後の全体サイズが最小になる`k`を選ぶ。
pub(crate) fn choose_optimal_k(counts: &[u32]) -> u8 {
    const MAX_RICE_K: usize = 12;

    let mut bits = [0u64; MAX_RICE_K + 1];
    for &count in counts {
        let value = encode_count(count);
        for (k, total) in bits.iter_mut().enumerate() {
            *total += (value >> k) as u64 + 1 + k as u64;
        }
    }
    bits.iter()
        .enumerate()
        .min_by_key(|&(_, total)| total)
        .unwrap()
        .0 as u8
}

#[inline(always)]
fn encode_count(count: u32) -> u32 {
    zigzag_encode(count as i32 - COUNT_CENTER)
}

#[inline(always)]
pub(super) fn decode_count(value: u32) -> u32 {
    (COUNT_CENTER + zigzag_decode(value)) as u32
}

// NOTE: 実測の結果、16を中央とするのが最適だった
const COUNT_CENTER: i32 = 16;

#[inline(always)]
fn zigzag_encode(value: i32) -> u32 {
    ((value << 1) ^ (value >> 31)) as u32
}

#[inline(always)]
fn zigzag_decode(value: u32) -> i32 {
    ((value >> 1) as i32) ^ -((value & 1) as i32)
}

#[cfg(test)]
mod tests {
    use super::{decode_count, encode_count};

    #[test]
    fn count_mapping_is_centered_at_sixteen() {
        assert_eq!(
            [16, 15, 17, 14, 18, 0, 32].map(encode_count),
            [0, 1, 2, 3, 4, 31, 32]
        );
        assert_eq!(
            [0, 1, 2, 3, 4, 31, 32].map(decode_count),
            [16, 15, 17, 14, 18, 0, 32]
        );
    }
}
