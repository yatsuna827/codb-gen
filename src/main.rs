//! ポケモンコロシアム「とにかくバトル」seed特定用の圧縮LightDB生成ツール。
//!
//! チーム生成ロジックはsrc/teamgen.rsを参照(PokemonCoRNGLibraryと一致)。
//! 観測1回 = 組み合わせ画面のチーム生成1回分であり、コードは
//! COMトレーナー名(3) × 自チーム(8) = 24通り。
//! 8回観測して先頭1回を捨て、残り7回(c1..c7)をキーとして
//! 1回のチーム生成適用後に到達可能な状態(像)のみを対象に検索する(LightDBの意味論)。
//!
//! 圧縮LightDB(単一ファイル、gen-light / query / verify-light / convert): src/clight.rs参照。

mod clight;
mod lcg;
mod teamdef;
mod teamgen;
mod teamgen_optimized;

use std::path::{Path, PathBuf};

use teamgen::generate_team;

// ---------------------------------------------------------------------------

/// C#ライブラリとの突き合わせ用: 1回のチーム生成の(seed, code, 生成後seed)を出力する
fn selftest(n: u32) {
    for i in 0..n {
        let seed = i.wrapping_mul(0x9E3779B9);
        let mut s = seed;
        let code = generate_team(&mut s);
        println!("{:08X} {:02} {:08X}", seed, code, s);
    }
}

/// 10進数または"0x"/"0X"接頭辞付き16進数を受理する数値パーサ。
fn parse_num(s: &str) -> u64 {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).expect("invalid hex number")
    } else {
        s.parse().expect("invalid number")
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let usage = "usage:
  codb-gen gen-light --out <FILE> [--limit <N>] [--threads <N>]
  codb-gen query <FILE> (<c1..c7> | <c0..c7> | --from-seed <HEXSEED>)
  codb-gen selftest [count]
  codb-gen verify-light <FILE>
  codb-gen convert <IN> <OUT> <cuml|cnts>";

    match args.get(1).map(|s| s.as_str()) {
        Some("selftest") => {
            let n = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(64);
            selftest(n);
        }
        Some("verify-light") => {
            let file = args.get(2).expect(usage);
            clight::verify(Path::new(file));
        }
        Some("convert") => {
            let input = args.get(2).expect(usage);
            let output = args.get(3).expect(usage);
            let target = args.get(4).expect(usage);
            clight::convert(Path::new(input), Path::new(output), target);
        }
        Some("query") => {
            let file = args.get(2).expect(usage);
            let rest: Vec<&String> = args.iter().skip(3).collect();
            let codes: [u32; 7] = if rest.len() == 2 && rest[0] == "--from-seed" {
                // 既知seedから観測8回分を生成して検索する自己テスト。
                // c0(先頭観測)は捨て、期待値(8回生成後seed)との照合まで行う。
                let s0 = u32::from_str_radix(rest[1], 16).expect("hex seed");
                let mut s = s0;
                let mut cs = [0u32; 8];
                for c in cs.iter_mut() {
                    *c = generate_team(&mut s);
                }
                println!(
                    "seed={:08X} codes={:?} expect={:08X}",
                    s0,
                    cs,
                    s
                );
                let codes: [u32; 7] = cs[1..8].try_into().unwrap();
                let results = clight::query(Path::new(file), &codes);
                let hit = results.iter().any(|&(_, fin)| fin == s);
                for (v, fin) in &results {
                    println!("origin={:08X} result={:08X}", v, fin);
                }
                println!("{}", if hit { "HIT" } else { "MISS" });
                std::process::exit(if hit { 0 } else { 1 });
            } else {
                // コード列指定: 7個ならc1..c7、8個なら先頭を捨てる
                let mut cs: Vec<u32> = rest.iter().map(|s| s.parse().expect("code")).collect();
                if cs.len() == 8 {
                    cs.remove(0);
                }
                cs.as_slice().try_into().expect("need 7 or 8 codes")
            };
            let results = clight::query(Path::new(file), &codes);
            for (v, fin) in &results {
                println!("origin={:08X} result={:08X}", v, fin);
            }
            eprintln!("{} match(es)", results.len());
        }
        Some("gen-light") => {
            // limitは「seed 0を起点とする軌道の先頭limit位置」を意味する
            // (詳細はdocs/design-orbit-scan.md参照)。limit = 2^32が全周期。
            let mut out: Option<PathBuf> = None;
            let mut limit: u64 = 1 << 32;
            let mut threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
            let mut it = args.iter().skip(2);
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--out" => out = Some(PathBuf::from(it.next().expect("--out FILE"))),
                    "--limit" => limit = parse_num(it.next().expect("--limit N")),
                    "--threads" => threads = it.next().expect("--threads N").parse().expect("threads"),
                    _ => panic!("unknown arg: {}\n{}", a, usage),
                }
            }
            let out = out.expect(usage);
            clight::generate(&out, limit, threads);
        }
        _ => eprintln!("{}", usage),
    }
}
