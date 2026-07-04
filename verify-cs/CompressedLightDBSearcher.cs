using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using PokemonPRNG.LCG32.GCLCG;

namespace PokemonCOSeedDataBaseAPI
{
    /// <summary>
    /// 圧縮LightDB(単一ファイル、codb-gen/src/clight.rs参照)を検索するサンプル実装です。
    /// 実物のFullDBSearcher/LightDBSearcher(SeedSearcher.cs)と同じ生成ロジックを使い、
    /// C#側から性能を検証するためのハーネス用クラスです。
    ///
    /// 圧縮LightDBは「版数」ではなく対等な兄弟形式として**CUML(累積形式)**と**CNTS(個数形式)**の
    /// 2種類を持つ。新ヘッダ(リトルエンディアン、両形式共通)は次の64Bです:
    ///   +0  magic    [u8; 8] = "COLIGHT\0"
    ///   +8  format   [u8; 4] = "CUML" または "CNTS"
    ///   +12 version  u32 = 1
    ///   +16 entry_count u64
    ///   +24 表セクションオフセット u64 (= 64。CUMLなら累積和表、CNTSなら個数列)
    ///   +32 部分seed配列オフセット u64
    ///   +40 checksum u64(検索では使わない)
    /// 部分seed配列(エントリ順にu16、起点seedの上位16bit)は両形式共通です。
    ///
    /// - CUML: プレフィックスP(24^5通り)ごとのエントリ数の累積和(u32)をそのまま並べたテーブル
    ///   (31,850,496バイト)。オープン時のロードは不要で、検索時に表を2箇所seekして
    ///   `表[P-1]`と`表[P]`を読むだけで区間 [lo, hi) が求まる(P=0のときlo=0)。
    /// - CNTS: プレフィックスPごとのエントリ数を6bitずつLSBファーストでビットパックした個数列
    ///   (ちょうど5,971,968バイト)。オープン時に個数列を全ロードし、256プレフィックスごとの
    ///   累積値アンカー(要素数⌈24^5/256⌉=31,104)を1回の走査で構築する(アンカー方式)。
    ///   検索時はアンカーから対象Pまでの残り(最大255個)だけ個数列を加算して区間を得る。
    ///
    /// 旧ヘッダ(version 1/2/3、magic直後がversionフィールドの形式)は非対応であり、
    /// わかりやすいエラーを投げて拒否します。
    /// </summary>
    public sealed class CompressedLightDBSearcher : IDisposable
    {
        enum Format { Cuml, Cnts }

        const long P_COUNT = 24L * 24 * 24 * 24 * 24; // 24^5 = 7,962,624
        const long HEADER_LEN = 64;
        const uint VERSION = 1;
        const long COUNTS_BYTES = (P_COUNT * 6) / 8; // 5,971,968
        const long CUML_BYTES = P_COUNT * 4; // 31,850,496
        const int ANCHOR_STRIDE = 256;
        static readonly byte[] MAGIC = System.Text.Encoding.ASCII.GetBytes("COLIGHT\0");
        static readonly byte[] FORMAT_CUML = System.Text.Encoding.ASCII.GetBytes("CUML");
        static readonly byte[] FORMAT_CNTS = System.Text.Encoding.ASCII.GetBytes("CNTS");

        readonly FileStream _fs;
        readonly BinaryReader _br;
        readonly Format _format;
        readonly long _offTable;
        readonly long _offSeeds;
        readonly long _entryCount;

        // CNTS専用: 個数列本体(末尾に番兵1バイトを付与したもの)。6bit値の取り出しに使う。
        readonly byte[] _counts;
        // CNTS専用: 256プレフィックスごとの累積エントリ数(anchor[j] = P<j*256の個数の総和)。
        readonly uint[] _anchor;

        public CompressedLightDBSearcher(string path)
        {
            var sw = Stopwatch.StartNew();

            _fs = new FileStream(path, FileMode.Open, FileAccess.Read);
            _br = new BinaryReader(_fs);

            var magic = _br.ReadBytes(8);
            if (!magic.SequenceEqual(MAGIC))
                throw new Exception($"bad magic: {path} はCOLIGHT形式ではありません。");

            var formatTag = _br.ReadBytes(4);
            if (formatTag.SequenceEqual(FORMAT_CUML)) _format = Format.Cuml;
            else if (formatTag.SequenceEqual(FORMAT_CNTS)) _format = Format.Cnts;
            else
            {
                // 旧ヘッダ(v1/v2/v3)はmagicの直後がu32のversionフィールドであり、
                // formatタグに相当する位置には小さい整数(1/2/3)がリトルエンディアンで入っている。
                // そのまま文字列化すると制御文字になり判読できないため、hexで示す。
                var legacyVersion = BitConverter.ToUInt32(formatTag, 0);
                var hex = string.Join(" ", formatTag.Select(b => b.ToString("X2")));
                if (legacyVersion is 1 or 2 or 3)
                    throw new Exception(
                        $"{path} は旧ヘッダ形式(v{legacyVersion})のCOLIGHTファイルです。" +
                        "本実装は新ヘッダ(CUML/CNTS、version=1)のみ対応しています。" +
                        "codb-gen側でCUMLまたはCNTSへ変換してから指定してください。");
                throw new Exception(
                    $"unsupported format tag (bytes: {hex}): {path} は新ヘッダ(CUML/CNTS)形式ではありません。");
            }

            var version = _br.ReadUInt32();
            if (version != VERSION)
                throw new Exception(
                    $"unsupported version {version} (expected {VERSION}): {path} は旧ヘッダ形式(v1/v2/v3)の" +
                    "可能性があります。新ヘッダ(CUML/CNTS、version=1)のファイルを指定してください。");

            _entryCount = (long)_br.ReadUInt64();
            _offTable = (long)_br.ReadUInt64();
            _offSeeds = (long)_br.ReadUInt64();
            _br.ReadUInt64(); // checksum(検索では使わない)

            if (_offTable != HEADER_LEN)
                throw new Exception("bad table offset");

            var tableBytes = _format == Format.Cuml ? CUML_BYTES : COUNTS_BYTES;
            if (_offSeeds != _offTable + tableBytes)
                throw new Exception("bad seeds offset");

            if (_format == Format.Cnts)
            {
                // 個数列を一括ロード(約5.7MB)。末尾に番兵1バイトを足しておくことで、
                // 最後のプレフィックスを読むときもbyte境界を跨ぐ2バイト読み出しが安全になる。
                _fs.Seek(_offTable, SeekOrigin.Begin);
                _counts = new byte[COUNTS_BYTES + 1];
                var read = _fs.Read(_counts, 0, (int)COUNTS_BYTES);
                if (read != COUNTS_BYTES) throw new Exception("failed to read counts section");
                _counts[COUNTS_BYTES] = 0; // 番兵

                // アンカー構築: 全P(約800万)を1回走査し、256個ごとの累積値を記録する。
                var anchorCount = (P_COUNT + ANCHOR_STRIDE - 1) / ANCHOR_STRIDE; // 31,104
                _anchor = new uint[anchorCount];
                {
                    ulong acc = 0;
                    int nbits = 0;
                    long byteIdx = 0;
                    long sum = 0;
                    for (long p = 0; p < P_COUNT; p++)
                    {
                        if ((p & (ANCHOR_STRIDE - 1)) == 0) _anchor[p >> 8] = (uint)sum;
                        while (nbits < 6)
                        {
                            acc |= (ulong)_counts[byteIdx] << nbits;
                            byteIdx++;
                            nbits += 8;
                        }
                        var v = (uint)(acc & 0x3F);
                        acc >>= 6;
                        nbits -= 6;
                        sum += v;
                    }
                }
            }
            // CUMLは何もロードしない(検索時に表をseekするだけ)。

            sw.Stop();
            Console.WriteLine($"[open] format={_format} load+build: {sw.Elapsed.TotalMilliseconds:F2}ms");
        }

        /// <summary>
        /// 個数列から6bit値(プレフィックスPのエントリ数)を1件だけ取り出します。
        /// bit=P*6として2バイトをまたいで読み、shift後に下位6bitを取り出す(番兵があるので
        /// 末尾のPでも範囲外アクセスにならない)。
        /// </summary>
        uint GetCount(long p)
        {
            var bit = p * 6;
            var byteIdx = bit >> 3;
            var shift = (int)(bit & 7);
            var v = (uint)(_counts[byteIdx] | (_counts[byteIdx + 1] << 8)) >> shift;
            return v & 0x3F;
        }

        /// <summary>
        /// (CNTS)プレフィックスPに対応するエントリ範囲[lo, hi)を、アンカーからの残差走査で求めます。
        /// anchor[P>>8]がP&~0xFF(256の倍数)時点の累積値なので、そこからP-1まで加算するだけでよく、
        /// 走査量は最大255件に収まります。
        /// </summary>
        (uint lo, uint hi) GetRangeCnts(long p)
        {
            var anchorIdx = p >> 8;
            var start = _anchor[anchorIdx];
            for (var q = anchorIdx << 8; q < p; q++)
            {
                start += GetCount(q);
            }
            var cnt = GetCount(p);
            return (start, start + cnt);
        }

        /// <summary>
        /// (CUML)プレフィックスPに対応するエントリ範囲[lo, hi)を、累積和表を2箇所seekして求めます。
        /// 表[i]は「P&lt;=iであるエントリ数の総和」なので、hi=表[P]、lo=表[P-1](P=0なら0)です。
        /// </summary>
        (uint lo, uint hi) GetRangeCuml(long p)
        {
            var hiVal = ReadUInt32At(_offTable + p * 4);
            var loVal = p == 0 ? 0u : ReadUInt32At(_offTable + (p - 1) * 4);
            return (loVal, hiVal);
        }

        uint ReadUInt32At(long offset)
        {
            _fs.Seek(offset, SeekOrigin.Begin);
            return _br.ReadUInt32();
        }

        /// <summary>
        /// 観測8回分の(トレーナー名, 自チーム)から起点seed候補を検索します。
        /// 先頭(観測1回目、c0)は起点seed特定には使わず、残り7回(c1..c7)で照合します。
        /// 戻り値は現行LightDBSearcherと同じ「8回生成後(=c1..c7の7回生成後)のseed」です。
        /// </summary>
        public IEnumerable<uint> Search((PlayerName playerNameIndex, BattleTeam teamIndex)[] keys)
        {
            if (keys.Length != 8) throw new Exception("Number of search keys must be 8.");

            var codes = keys.Skip(1)
                .Select(k => (uint)k.playerNameIndex * 8 + (uint)k.teamIndex)
                .ToArray(); // c1..c7 (7要素)

            // P = c1*24^4 + c2*24^3 + c3*24^2 + c4*24 + c5
            long p = 0;
            for (int i = 0; i < 5; i++) p = p * 24 + codes[i];

            var (lo, hi) = _format == Format.Cuml ? GetRangeCuml(p) : GetRangeCnts(p);
            if (hi <= lo) yield break;

            // 該当範囲の部分seed配列(u16×件数)をまとめて読む
            var n = (int)(hi - lo);
            _fs.Seek(_offSeeds + lo * 2, SeekOrigin.Begin);
            var buf = _br.ReadBytes(n * 2);

            for (int i = 0; i < n; i++)
            {
                uint hi16 = (uint)(buf[i * 2] | (buf[i * 2 + 1] << 8));

                // 下位16bitを全探索し、前向きシミュレートで照合する
                for (uint x = 0; x < 0x10000; x++)
                {
                    var v = (hi16 << 16) | x;
                    var s = v;
                    if (TryGenerateAllChecked(ref s, codes))
                        yield return s;
                }
            }
        }

        /// <summary>
        /// c1..c7を順に照合しながら7回分のチーム生成を進めます。
        /// 全部一致すればtrueを返し、その時点でsは7回生成後のseedになります。
        /// </summary>
        static bool TryGenerateAllChecked(ref uint s, uint[] codes)
        {
            foreach (var code in codes)
            {
                if (!GenerateTeamChecked(ref s, code)) return false;
            }
            return true;
        }

        /// <summary>
        /// RentalTeamRank.GenerateCode相当の1回分のチーム生成を、期待コードと照合しながら進めます。
        /// 早期棄却が性能の要です:
        ///   (a) 自チームindexが確定した時点(敵チーム生成より前)で不一致なら棄却
        ///   (b) トレーナー名が確定した時点(自チーム生成より前)で不一致なら棄却
        /// </summary>
        static bool GenerateTeamChecked(ref uint s, uint code)
        {
            var wantName = code / 8;
            var wantTeam = code % 8;

            var enemyTeamIndex = s.GetRand() & 0x7;
            uint playerTeamIndex;
            do { playerTeamIndex = s.GetRand() & 0x7; } while (playerTeamIndex == enemyTeamIndex);
            if (playerTeamIndex != wantTeam) return false; // (a)

            var enemyTSV = s.GetRand() ^ s.GetRand();
            foreach (var poke in BattleTeamUltimate.UltimateTeams[enemyTeamIndex])
                GenerateSlot(ref s, poke, enemyTSV);

            var playerNameIndex = s.GetRand(3);
            if (playerNameIndex != wantName) return false; // (b)

            var playerTSV = s.GetRand() ^ s.GetRand();
            foreach (var poke in BattleTeamUltimate.UltimateTeams[playerTeamIndex])
                GenerateSlot(ref s, poke, playerTSV);

            return true;
        }

        /// <summary>
        /// GCSlot.Use相当: dummyPID(2)+IVs(2)+ability(1)の5回消費のあと、
        /// 性別・性格・色回避(TSVに対する色違い判定)の条件を満たすPIDが出るまで再抽選します。
        /// </summary>
        static void GenerateSlot(ref uint s, (GenderRatio GenderRatio, Gender fixedGender, Nature fixedNature) poke, uint tsv)
        {
            s.Advance(5);
            while (true)
            {
                var hi = s.GetRand();
                var lo = s.GetRand();
                var pid = (hi << 16) | lo;
                if (GetGender(pid, poke.GenderRatio) != poke.fixedGender) continue;
                if (pid % 25 != (uint)poke.fixedNature) continue;
                if ((hi ^ lo ^ tsv) < 8) continue; // 色回避
                break;
            }
        }

        static Gender GetGender(uint pid, GenderRatio ratio)
        {
            if (ratio == GenderRatio.Genderless) return Gender.Genderless;
            return (pid & 0xFF) < (uint)ratio ? Gender.Female : Gender.Male;
        }

        public void Dispose()
        {
            _br.Dispose();
            _fs.Dispose();
        }
    }
}
