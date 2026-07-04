# 引き継ぎメモ: とにかくバトルDB(FullDB / LightDB)生成のRust移植

## 概要

ポケモンコロシアム「とにかくバトル(シングル最強)」による現在seed特定用データベースの生成コードをRustで実装したもの(本リポジトリ)。
現行576ファイル形式(FullDB/LightDB)の生成に加え、単一ファイルの圧縮LightDB
(CUML/CNTSの2形式、`gen-light` / `query` / `verify-light` / `convert`)を生成・検索できる
(コアのチーム生成ロジックは`src/teamgen.rs`、圧縮フォーマットは`src/clight.rs`)。

- 圧縮フォーマットの詳細設計: `docs/design-compressed-lightdb.md`
  (正確なファイルレイアウトは`src/clight.rs`冒頭のdocコメントを正とする)
- 不採用の代替案: `docs/alternative-tmto.md`

## 関連リポジトリ・データの所在

| 対象 | 場所 |
|---|---|
| 検索側C#(正のキー定義) | `PokemonCOSeedDataBaseAPI`(SeedSearcher.cs, BattleTeam.cs) |
| 生成ロジックの正(C#) | `PokemonCoRNGLibrary`(RentalTeamRank / GCSlot) |
| 旧C++生成器(参考・誤りあり) | `CODatabase/CODataBase.cpp` |
| 旧C#生成パイプライン(参考) | `CODB_test` |
| 現物LightDB(基準データ) | 576ファイル、計1,024,761,320B、128,095,165エントリ |

## DBの仕様

観測1回 = COMトレーナー名(3) × 自チーム(8) = 24通り。敵チームは画面に出ないため使えない。

- **FullDB**: 全2^32状態が対象。状態sから7バトル分のコード c0..c6 を計算し、
  ファイル `{c5+c6*24}.bin` に `seedKey = c4 + c3·24 + c2·24² + c1·24³ + c0·24⁴` の昇順で
  **seed(u32 LE)のみ**を格納。約16GiB
- **LightDB**: 8回観測して先頭1回を捨てる方式。1バトル適用後に到達可能な状態(像)のみが対象。
  像の状態vから7バトル分 c1..c7 を計算し、ファイル `{c1+c2*24}.bin` に
  `seedKey = c3 + c4·24 + c5·24² + c6·24³ + c7·24⁴` の昇順で
  **(seedKey u32, 生成後seed u32)(LE)** を格納。値は「7バトル生成後の状態」であり
  検索結果をそのまま現在seedとして使える。約1GiB
- 像の密度は全状態の**約3.80%**(distinct像数163,104,481/2^32)。
  理論上は末尾採用PIDの条件(性格×性別の和 = 705/6400 ≈ 0.110)が上界で、逆向きパースの完走率
  (臨界分岐過程)が掛かる。この完走率は像密度基準で実測**約0.345**
- 現物LightDBおよび本ツールの成果物のエントリ数**128,095,165**は像の総数ではなく、
  **「(キーK, 7回生成後seed)のdistinctペア数」**である。異なる像v1≠v2がチーム生成1回で
  同じ状態に合流し(f(v1)=f(v2))、かつ1回目のコードも同じ場合、行(seedKey, 生成後seed)は
  完全に同一になるため、dedup後のエントリ数は像の総数(全状態の約3.80%)より少なくなる。
  検索出力(生成後seed)としてはこの重複除去は無劣化(同一結果が2回返るところを1回にするだけ)

## 圧縮LightDB(CUML/CNTS)の成果物と検証

成果物: `out/lightdb-cuml.cldb`(288,040,890バイト)・`out/lightdb-cnts.cldb`
(262,162,362バイト)。いずれも128,095,165エントリで、現物LightDBと同一集合。

検証済み事項:

- `verify-light`でエントリ数・チェックサム・単調性がOK(両ファイル共)
- `convert`によるCUML/CNTS相互変換は往復(cnts→cuml→cnts)でバイト完全一致
- ランダム20seedの観測列で`query`が全件HITし、同一20seedをC#実物Searcher
  (`PokemonCOSeedDataBaseAPI`、現物LightDB)で検索した結果と全件一致
- 検索側C#サンプル実装`verify-cs/CompressedLightDBSearcher.cs`(ヘッダのformatタグから
  CUML/CNTSを自動判別し、プレフィックス表参照+下位16bit全探索・前向きシミュレートで
  `Search((PlayerName,BattleTeam)[8]) -> IEnumerable<uint>` を実装)でも既知seed検索が全件HIT
- クエリレイテンシ実測(Intel N150、`verify-cs bench`、プロセス起動込み):
  CUMLはN=300で平均約395ms/中央値約388ms/最大約781ms(追試N=50で平均約414ms/最大約641ms)、
  CNTSはN=100で平均423ms/中央値417ms/最大831ms。いずれも要件「1クエリ1秒以内」を満たす

## 576ファイル形式(FullDB/LightDB)の検証状態

- バトル生成コア: `codb-gen selftest 256` と `verify-cs/`(C#ハーネス、PokemonCoRNGLibrary の
  `GenerateCode` を呼ぶ)の出力が256件完全一致
- smoke規模の生成データに対し実物Searcher(`PokemonCOSeedDataBaseAPI`のFullDBSearcher /
  LightDBSearcher)で既知seedの観測列を検索し、期待した生成後seed(full=7バトル進行後、
  light=8バトル進行後)が得られることを確認済み。キーのパッキング順序・観測アラインメントの
  解釈に問題がないことが確定している
- **576ファイル形式の本番生成(全2^32状態)は未実施**。圧縮LightDBの本番生成・検証を
  優先したため着手していない。このマシンは4スレッドで、シミュレーション約6時間+ソートの見込み。
  出力先を`Documents\codb`直下にすると現物LightDBと衝突するので別ディレクトリにすること

## 検証の再実行手順

バトル生成コアの一致確認:

```
codb-gen selftest 256 > rs_out.txt
cd verify-cs && dotnet run -c Release -- 256 > cs_out.txt
```

576ファイル形式の既知seed検索(実物Searcherを使用):

```
cd verify-cs
dotnet run -c Release -p:RestoreAdditionalProjectSources=<ローカルNuGetフォルダ> -- search <DIR>\FullDB full <hexseed>...
dotnet run -c Release -p:RestoreAdditionalProjectSources=<ローカルNuGetフォルダ> -- search <DIR>\LightDB light <hexseed>...
```

圧縮LightDB(CUML/CNTS)の既知seed検索・ベンチマーク:

```
cd verify-cs
dotnet run -c Release -p:RestoreAdditionalProjectSources=<ローカルNuGetフォルダ> -- csearch <cldbfile> <hexseed>...
dotnet run -c Release -p:RestoreAdditionalProjectSources=<ローカルNuGetフォルダ> -- bench <cldbfile> <N>
```

※ PokemonCoRNGLibrary の依存パッケージ(PokemonPRNG.LCG32.*, PokemonStandardLibrary*)は
  nuget.org に無いため、.nupkg を置いたローカルフォルダを `-p:RestoreAdditionalProjectSources` で渡す
  (verify-cs/nuget.config だけでは参照先プロジェクトのrestoreに効かない)。
※ PokemonCOSeedDataBaseAPI は csproj が無いソースのみのため、cshtest.csproj が `Compile Include` で直接取り込む。

## 576ファイル形式を本番生成した後にやること

1. `codb-gen verify <DIR>\LightDB --pairs` / `verify <DIR>\FullDB`
2. LightDBのエントリ数が **128,095,165** と一致するか確認(現物と同一集合になるはず。不一致なら生成ロジック差分を疑う。ただし現物は色回避や性別比の扱いが不明な旧ライブラリ製の可能性があるので、軽微な差は「現行ライブラリが正」)
3. 新旧Searcherでのクエリ突き合わせ

## 未着手事項

- 576ファイル形式の本番生成(必要性はユーザー判断)
- 検索側アプリへの圧縮LightDBの本組み込み(サンプル実装と性能確認は完了。アプリへの組み込みは別途)
- コミット
