use crate::core::teamgen::generate_team;

// 元実装との照合を行うサブコマンド

pub fn selftest(n: u32) {
    for i in 0..n {
        let seed = i.wrapping_mul(0x9E3779B9);
        let mut s = seed;
        let code = generate_team(&mut s);
        println!("{:08X} {:02} {:08X}", seed, code, s);
    }
}
