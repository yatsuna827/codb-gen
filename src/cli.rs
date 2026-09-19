mod query;
mod selftest;
mod verify;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use crate::core::teamgen::generate_team;

use query::query;
use selftest::selftest;
use verify::verify;

pub(crate) fn run() {
    let args: Vec<String> = std::env::args().collect();
    let usage = "usage:
  codb-gen gen --out <FILE> [--limit <N>] [--threads <N>]
  codb-gen query <FILE> (<c1..c7> | <c0..c7> | --from-seed <HEXSEED>)
  codb-gen selftest [count]
  codb-gen verify <FILE>";

    match args.get(1).map(|s| s.as_str()) {
        Some("gen") => {
            // MEMO: limitは生成対象を減らして高速にデバッグを回すための補助オプション
            let mut out: Option<PathBuf> = None;
            let mut limit: Option<u64> = None;
            let mut threads = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(8);
            let mut it = args.iter().skip(2);
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--out" => out = Some(PathBuf::from(it.next().expect("--out FILE"))),
                    "--limit" => limit = Some(parse_num(it.next().expect("--limit N"))),
                    "--threads" => {
                        threads = it.next().expect("--threads N").parse().expect("threads")
                    }
                    _ => panic!("unknown arg: {}\n{}", a, usage),
                }
            }
            let out = out.expect(usage);
            crate::generate::generate(&out, limit, threads);
        }

        Some("query") => {
            let file = args.get(2).expect(usage);
            let rest: Vec<&String> = args.iter().skip(3).collect();
            if rest.len() == 2 && rest[0] == "--from-seed" {
                // from-seedの場合
                // 指定されたseedから8回分の生成結果を計算して検索処理を実行し、ヒットするか検証する
                let s0 = u32::from_str_radix(rest[1], 16).expect("hex seed");
                let mut s = s0;
                let mut cs = [0u32; 8];
                for c in cs.iter_mut() {
                    *c = generate_team(&mut s);
                }
                println!("seed={:08X} codes={:?} expect={:08X}", s0, cs, s);

                let codes: [u32; 7] = cs[1..8].try_into().unwrap();
                let results = query(Path::new(file), &codes);
                let hit = results.iter().any(|&(_, fin)| fin == s);
                for (v, fin) in &results {
                    println!("origin={:08X} result={:08X}", v, fin);
                }
                println!("{}", if hit { "HIT" } else { "MISS" });

                std::process::exit(if hit { 0 } else { 1 });
            } else {
                // 検索コードが指定された場合、それを使って検索した結果を返す
                // 8回分のコードが指定された場合は1回目を落として後ろ7回分だけを使う
                let mut cs: Vec<u32> = rest.iter().map(|s| s.parse().expect("code")).collect();
                if cs.len() == 8 {
                    cs.remove(0);
                }
                let codes: [u32; 7] = cs.as_slice().try_into().expect("need 7 or 8 codes");

                let results = query(Path::new(file), &codes);
                for (v, fin) in &results {
                    println!("origin={:08X} result={:08X}", v, fin);
                }
                eprintln!("{} match(es)", results.len());
            };
        }

        Some("selftest") => {
            let n = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(64);
            selftest(n);
        }

        Some("verify") => {
            let file = args.get(2).expect(usage);
            verify(Path::new(file));
        }

        _ => eprintln!("{}", usage),
    }
}

fn parse_num(s: &str) -> u64 {
    // 10進数、または、"0x"/"0X"から始まる16進数を受け付ける
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).expect("invalid hex number")
    } else {
        s.parse().expect("invalid number")
    }
}
