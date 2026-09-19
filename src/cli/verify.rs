use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::format::reader::{read_header, CountsReader};
use crate::format::{fnv1a64, FNV_INIT, HEADER_LEN, P_COUNT};

// dbファイルが正しく生成されているかを検証するサブコマンド
// 検証する項目は以下の通り
// - マジックナンバー
// - バージョン
// - ファイルサイズ
// - チェックサム
// - テーブルセクションの大雑把な整合性
//   - 総和がentry_countと一致すること

pub fn verify(path: &Path) {
    let mut f = File::open(path).expect("open db");
    let h = read_header(&mut f);
    let n = h.entry_count;
    f.seek(SeekFrom::Start(HEADER_LEN)).expect("seek");
    let mut checksum = FNV_INIT;

    let table_len = (h.off_seeds - HEADER_LEN) as usize;
    let mut counts_buf = vec![0u8; table_len];
    f.read_exact(&mut counts_buf).expect("read counts");
    fnv1a64(&mut checksum, &counts_buf);

    let mut reader = CountsReader::new(&counts_buf, h.rice_k);
    let total: u64 = (0..P_COUNT).map(|_| reader.read() as u64).sum();
    assert!(total == n, "counts total != entry_count");

    let mut seeds = vec![0u8; (n * 2) as usize];
    f.read_exact(&mut seeds).expect("read seeds");
    fnv1a64(&mut checksum, &seeds);

    assert!(checksum == h.checksum, "checksum mismatch");

    println!("{}: {} entries, OK", path.display(), n);
}
