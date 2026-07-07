# 引き継ぎメモ: 圧縮LightDB生成のRust移植

## 概要

ポケモンコロシアム「とにかくバトル(シングル最強)」による現在seed特定用データベースの生成コードをRustで実装したもの(本リポジトリ)。
単一ファイルの圧縮LightDB(CUML/CNTSの2形式、`gen-light` / `query` / `verify-light` / `convert`)を生成・検索できる(コアのチーム生成ロジックは`src/teamgen.rs`、圧縮フォーマットは`src/clight.rs`)。
`gen-light`の生成はLCG軌道順走査+スライディングウィンドウ方式で、生成中は常時オンライン自己検査(状態の下位12bitが0の像をその場で素直な7回生成と照合)が有効になっている。

- 圧縮フォーマットの詳細設計: `docs/design-compressed-lightdb.md`(正確なファイルレイアウトは`src/clight.rs`冒頭のdocコメントを正とする)
- 生成方式(LCG軌道順走査+スライディングウィンドウ)の詳細設計: `docs/design-orbit-scan.md`
- 不採用の代替案: `docs/alternative-tmto.md`

## 関連リポジトリ・データの所在

| 対象 | 場所 |
|---|---|
| 検索側C#(正のキー定義) | `PokemonCOSeedDataBaseAPI`(SeedSearcher.cs, BattleTeam.cs) |
| 生成ロジックの正(C#) | `PokemonCoRNGLibrary`(RentalTeamRank / GCSlot) |
| 現物LightDB(基準データ) | 128,095,165エントリ(圧縮LightDBの検証における基準集合) |

## DBの仕様

観測1回 = COMトレーナー名(3) × 自チーム(8) = 24通り。
敵チームは画面に出ないため使えない。

- 8回観測して先頭1回を捨てる方式。チーム生成1回適用後に到達可能な状態(像)のみが対象となる。像の状態から7回分のコードc1..c7を計算し、これをキーとする。検索結果は「7回生成後の状態」であり、そのまま現在seedとして使える。
- 像の密度は全状態の**約3.80%**(distinct像数163,104,481/2^32)。理論上は末尾採用PIDの条件(性格×性別の和 = 705/6400 ≈ 0.110)が上界で、逆向きパースの完走率(臨界分岐過程)が掛かる。この完走率は像密度基準で実測**約0.345**。
- 現物LightDBおよび本ツールの成果物のエントリ数**128,095,165**は像の総数ではなく、**「(キー, 7回生成後seed)のdistinctペア数」**である。異なる像v1≠v2がチーム生成1回で同じ状態に合流し(f(v1)=f(v2))、かつ1回目のコードも同じ場合、行(キー, 生成後seed)は完全に同一になるため、dedup後のエントリ数は像の総数(全状態の約3.80%)より少なくなる。検索出力(生成後seed)としてはこの重複除去は無劣化(同一結果が2回返るところを1回にするだけ)。

## 圧縮LightDB(CUML/CNTS)の成果物と検証

成果物: `out/lightdb-cuml.cldb`(288,040,890バイト)・`out/lightdb-cnts.cldb`(260,314,978バイト)。
いずれも128,095,165エントリで、現物LightDBと同一集合。
CNTSの個数列セクションはzigzag+Rice符号化で実装済み(パラメータk=2。生成時に全kを総当たりして符号長最小のkを選択し、ヘッダ+48にu8で格納)。
`out/lightdb-cnts-old6bit.cldb`は旧6bit固定幅方式の退避ファイルで現行コードでは読めない(削除するかはユーザー判断)。

検証済み事項:

- バトル生成コア: `codb-gen selftest 256`と`verify-cs/`(C#ハーネス、PokemonCoRNGLibraryの`GenerateCode`を呼ぶ)の出力が256件完全一致。
- LCG軌道順走査+スライディングウィンドウ方式による全周期(2^32)の生成出力が、既存成果物`out/lightdb-cnts.cldb`とバイト完全一致(2026-07-05確認。オンライン自己検査は39,896件すべて通過)。所要時間はこのマシン(Intel N150、4スレッド)で約37分(走査2192秒+ソート5秒+書き出し2秒)。
- `verify-light`でエントリ数・チェックサム・単調性がOK(両ファイル共)。
- `convert`によるCUML/CNTS相互変換は往復(cnts→cuml→cnts)でバイト完全一致。
- ランダム20seedの観測列で`query`が全件HITし、同一20seedをC#実物Searcher(`PokemonCOSeedDataBaseAPI`、現物LightDB)で検索した結果と全件一致。
- 検索側C#サンプル実装`verify-cs/CompressedLightDBSearcher.cs`(ヘッダのformatタグからCUML/CNTSを自動判別し、プレフィックス表参照+下位16bit全探索・前向きシミュレートで`Search((PlayerName,BattleTeam)[8]) -> IEnumerable<uint>`を実装)でも既知seed検索が全件HIT。
- クエリレイテンシ実測(Intel N150、`verify-cs bench`、プロセス起動込み):CUMLはN=300で平均約395ms/中央値約388ms/最大約781ms(追試N=50で平均約414ms/最大約641ms)、CNTSはN=100(先頭1件除く99件集計)で平均399.97ms/中央値392.55ms/最大811.05ms(オープン時のRice復号+チェックポイント構築は約218ms)。いずれも要件「1クエリ1秒以内」を満たす。

## 生成の高速化(実装済み)

実測(このマシン、旧方式の走査部2192秒)に基づく初期整理:

- 走査コストの実体は各位置でのチーム生成(約2µs=約5.15サイクル/乱数)で、これはLCGの逐次乗算チェーン(4サイクル/ステップ)のレイテンシ下限にほぼ張り付いていた。ウィンドウ管理のオーバーヘッドは約12%。設計時に懸念したビットマップのランダムアクセスは実際には生成1回のコストの5%程度でしかなかった(旧方式のPhase1と新方式の走査がほぼ同速な理由)。
- 採用済み: `.cargo/config.toml`の`target-cpu=native`(約2%改善。gitignore対象のローカル設定)。`lto="fat"`と`codegen-units=1`はCargo.tomlに設定済み。
- 不採用: 複数レーンのロックステップインターリーブ(ILP狙い)。L=4/8/16すべてで逆に大幅悪化。手書き4レーン展開でもスカラー比25%悪化で、このコア(Gracemont)では乗算チェーンの並行実行によるレイテンシ隠蔽が効かないと判断。経緯は`docs/design-orbit-scan.md`の検討事項に記録。

**採用済み(2026-07-05): 乱数値の共有 + 再抽選のバッチ走査(SIMD)**。
走査部が約28%高速化した(このマシン・4スレッド・全周期`limit=2^32`で走査2192秒 → 1584秒、全体約37分 → 約26.5分。部分実行`limit=0x2000000`では走査16.9〜17.0秒 → 12.7〜13.1秒)。
出力はバイト完全不変(全周期実行が実物`out/lightdb-cnts.cldb`とバイト完全一致。自己検査39,896件すべて通過。部分実行でもスレッド数1/3/4で既存成果物とバイト一致を確認)。
設計の詳細は`docs/design-orbit-scan.md`。
要点:

- **乱数値の共有(テーブル駆動生成)**: 位置jの生成が消費する乱数列s_{j+1}..s_{j+n}の上位16bitはリングバッファ`ring.s`に既に格納しているsそのもの。状態列の書き込みを生成位置より`LEAD`位置先行させ(1位置あたりLCG1ステップ)、チーム生成を`ring.s`読みのテーブル駆動(`teamgen::generate_team_from_table`)に変えて、生成内部の逐次LCG乗算チェーンを消した。ただし**これ単体ではパリティ止まり**だった(テーブル読みのロードレイテンシが乗算チェーンとほぼ同等)。素朴実装ではLLVMが後続の乱数消費位置をidxのアフィン式として先行計算し、それらを同時に多数保持するためレジスタが不足して逆に2.6倍悪化した。`get_unchecked`(境界チェック除去)で解消したが、真に効いたのは次項。
- **LEAD縮小(キャッシュ局所性)**: `LEAD`は当初「消費数nの上界(u16::MAX)以上」で`2^16`に取っていたが、fillが65,536先で書くため読むまでにキャッシュから追い出されていた。平均nに近い`LEAD=2^14`(16,384)へ縮め、fill(書き)と生成(読み)を時間的に近接させた。n>LEADの稀な位置は`generate_team_from_table`が`None`を返し、呼び出し側がスカラー(`generate_team_with_count`)で再計算するフォールバックで救う(`LEAD < RING-LAG=2^17`ならスロット再利用は安全、値を小さくするほど安全側)。LEAD=16,384ではフォールバック率0%。効果は数%でこのマシンの実測ノイズと同程度。
- **再抽選ループのバッチ走査(SIMD)**: gen_slotの再抽選を、テーブル経路専用に`B=16`候補ずつ連続スライス上でまとめて評価する形(`teamgen::gen_slot_table`)に書き換えた。受理判定をバッチ内で計算しビットマスクの最下位1bitで最初の受理候補を取る。リングの折り返しをまたぐバッチのみスカラーにフォールバックする。この形にするとレジスタ不足が起きず、かつLLVMが自動ベクトル化してAVX2(ymm、`pid % 25`の乗算逆数化・比較・マスクを8レーン)で走る。Bは8/16/32を実測してB=16が最良。手書きAVX2組み込みは自動ベクトル化で既にymm化されたため不要。
- スカラー経路(`generate_team`/`generate_team_checked`、selftest・query・オンライン自己検査が使う)は無変更のまま。テーブル経路の正しさはオンライン自己検査(テーブル合成をスカラー再計算と照合)と部分実行のバイト一致で担保している。

**追加採用(2026-07-07): 乱数値列のu16化(読み込みウィンドウの半減)**。
走査部がさらに高速化した(このマシン・4スレッド・部分実行`limit=0x2000000`で走査中央値12.9秒→10.9秒。全周期`limit=2^32`の実測で走査1,584秒→1,274秒(約19.6%短縮)、生成全体約26.5分→約21.3分)。
出力はバイト完全不変(全周期実行が実物`out/lightdb-cnts.cldb`とバイト完全一致。`limit=0x2000000`・`0x1000000`のsha256一致。自己検査も全通過)。設計の詳細は`docs/design-orbit-scan.md`「乱数値列のu16化」。
要点:

- テーブル駆動生成が読むのは状態sの**上位16bitだけ**(`gen_slot_table`/`next_tab`は`s >> 16`しか使わない)なので、上位16bitだけを持つ`ring.hi`(u16配列)を`ring.s`(u32)と並行して持ち(fillで同時書き込み)、ホットループは`ring.hi`を読むようにした。
- 効果はキャッシュ局所性。読む要素あたりが4B→2Bになり、`LEAD=2^14`の生存ウィンドウが約64KB(L2)→約32KB(L1D)、バッチ走査の連続32要素が128B(2ライン)→64B(1ライン)に収まる。`generate_team_from_table`/`gen_slot_table`/`next_tab`を`&[u16]`化し各`>> 16`を除去しただけで、読む値は同一なので出力不変。

## 検証の再実行手順

生成側の高速な検証(C#非依存、数秒〜十数秒):

```
cargo test
```

- `teamgen::tests::core_golden`: selftest相当の生成結果(4096件)のFNVハッシュを固定値と照合。値はC#参照実装と一致確認済みの現行挙動を固定したもの。生成コアが変わると落ちる。
- `teamgen::tests::generate_team_checked_consistency`: `generate_team_checked`が`generate_team`と整合(query側の照合の正しさ)。
- `teamgen::tests::table_path_matches_scalar`: テーブル駆動生成(乱数値共有の最適化経路)がスカラー生成と(code, n, 最終状態)完全一致。
- `clight::tests::thread_invariance_and_selfcheck`: 小limitでthreads=1と4の出力がバイト一致。実行中にgen-light内蔵のオンライン自己検査も駆動される。

生成のベンチと部分実行のsha256回帰チェック:

```
bash scripts/gen-check.sh
```

(limit=0x1000000・threads=4で生成し、sha256を埋め込み期待値と照合してphase1+2の最小/中央値を出力。
全周期のバイト一致は別途`gen-light`を全周期実行して`out/lightdb-cnts.cldb`と`cmp`する最終確認で行う)

バトル生成コアのC#参照実装との一致確認(C#環境が要る。golden定数の元になった確認):

```
codb-gen selftest 256 > rs_out.txt
cd verify-cs && dotnet run -c Release -- 256 > cs_out.txt
```

圧縮LightDB(CUML/CNTS)の既知seed検索・ベンチマーク:

```
cd verify-cs
dotnet run -c Release -p:RestoreAdditionalProjectSources=<ローカルNuGetフォルダ> -- csearch <cldbfile> <hexseed>...
dotnet run -c Release -p:RestoreAdditionalProjectSources=<ローカルNuGetフォルダ> -- bench <cldbfile> <N>
```

※ PokemonCoRNGLibraryの依存パッケージ(PokemonPRNG.LCG32.*, PokemonStandardLibrary*)はnuget.orgに無いため、.nupkgを置いたローカルフォルダを`-p:RestoreAdditionalProjectSources`で渡す(verify-cs/nuget.configだけでは参照先プロジェクトのrestoreに効かない)。
※ PokemonCOSeedDataBaseAPIはcsprojが無いソースのみのため、cshtest.csprojが`Compile Include`でCompressedLightDBSearcher.csが使う型定義(Enums.cs, BattleTeam.cs)だけを直接取り込む。

## 未着手事項

生成の高速化はひと通り出し切った。
残る候補は効果がこのマシンの実測ノイズ以下と判断して見送り:

- **手書きAVX2での再抽選走査(見送り)**: 自動ベクトル化で既にAVX2(ymm)化されており、手書き組み込みの上積みは不透明かつ高リスク(バイト一致の担保が難しい)なため不要と判断。
- **リング折り返しフォールバックの削減(見送り)**: バッチがRING境界(2^18位置に1回)をまたぐ生成でのみ発生し、頻度<0.5%で効果はノイズ以下。
- LEAD縮小によるキャッシュ局所性改善は実施済み(上記「生成の高速化」参照)。
