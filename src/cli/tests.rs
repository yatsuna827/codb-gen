use super::verify;
use crate::generate::generate;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// std::env::temp_dir()配下に、プロセスID+ナノ秒時刻でユニークなパスを作る
/// (並列テスト実行や再実行での衝突を避ける)。
fn unique_temp_path(tag: &str) -> PathBuf {
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("codb-gen-test-{}-{}-{}.cldb", tag, pid, nanos))
}

/// 生成の並列度不変性・オンライン自己検査・構造検証を1本で確認する。
/// - `generate(&p1, Some(LIMIT), 1)` と `generate(&p4, Some(LIMIT), 4)` を実行し、
///   出力バイト列が完全一致すること(スレッド数がバイト出力に影響しないこと)を確認する。
/// - 生成の実行中、gen内蔵のオンライン自己検査(scan_arc内、
///   テーブル駆動の結果をスカラー再計算と照合)が走り、不一致ならpanicする。
///   すなわちこのテスト自体が自己検査を駆動する。
/// - `verify(&p1)`がpanicしないこと(構造検証: ヘッダ・オフセット・チェックサム等)を確認する。
#[test]
fn thread_invariance_and_selfcheck() {
    // limitに対して固定のスキャン前後幅(PROLOGUE=2^16+LAG=2^17=196,608位置/スレッド)が
    // 支配的なため、limitを小さくしても際限なく速くなるわけではない。0x8000(=32768)は
    // 自己検査サンプル(origin下位12bit==0、期待値約8件)が実効的に発生しつつ、
    // cargo test全体を数秒〜十数秒に収めるために選んだ値。
    const LIMIT: u64 = 0x8000;

    let p1 = unique_temp_path("t1");
    let p4 = unique_temp_path("t4");

    generate(&p1, Some(LIMIT), 1);
    generate(&p4, Some(LIMIT), 4);

    let b1 = std::fs::read(&p1).expect("read p1 output");
    let b4 = std::fs::read(&p4).expect("read p4 output");
    assert_eq!(
        b1, b4,
        "generate() output must be byte-identical regardless of thread count"
    );

    // 構造検証(マジック/フォーマット/オフセット/チェックサム等)がpanicしないこと。
    verify(&p1);

    std::fs::remove_file(&p1).ok();
    std::fs::remove_file(&p4).ok();
}
