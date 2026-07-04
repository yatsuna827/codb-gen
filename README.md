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
