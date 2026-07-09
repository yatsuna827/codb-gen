# codb-gen

ポケモンコロシアムの「とにかくバトル」を用いたseed特定用のDBファイルを生成するCLIツールです。

DBの詳しい背景・仕様は`HANDOFF.md`、圧縮フォーマットや生成方式の設計は`docs/design-compressed-lightdb.md`・`docs/design-orbit-scan.md`を参照してください。

## ビルド

```
cargo build --release
```

DB生成をさらに高速化したい場合は、`.cargo/config.toml`を作成し、以下のローカル設定を適用してください。

```toml
[build]
rustflags = ["-C", "target-cpu=native"]
```

## 生成

```
./target/release/codb-gen.exe gen-light --out <FILE> [--limit <N>] [--threads <N>]
```

- `--out`: 出力ファイルパス(必須)。出力は圧縮LightDBのCNTS形式です。
- `--limit`: 走査する位置数です。省略時は`2^32`(全周期)になります。部分実行の動作確認には`0x1000000`などを指定してください(数値は10進または0x接頭辞の16進で指定できます)。
- `--threads`: スレッド数です。省略時は論理CPU数になります。

実行例:

```
./target/release/codb-gen.exe gen-light --out out/lightdb-cnts.cldb
```

## 検証

生成物の構造(エントリ数・チェックサム等)を検証できます。

```
./target/release/codb-gen.exe verify-light <FILE>
```

このほか、`cargo test`で生成コアの回帰テストを、`bash scripts/gen-check.sh`で部分実行のsha256回帰チェックとベンチを実行できます。
