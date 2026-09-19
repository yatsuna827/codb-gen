using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using PokemonPRNG.LCG32.GCLCG;

namespace PokemonCOSeedDataBaseAPI
{
    public sealed class CompressedLightDBSearcher : IDisposable
    {
        const long P_COUNT = 24L * 24 * 24 * 24 * 24;
        const long HEADER_LEN = 32;
        const uint VERSION = 1;
        const int CHECKPOINT_STRIDE = 256;
        const int COUNT_CENTER = 16;
        static readonly byte[] MAGIC = System.Text.Encoding.ASCII.GetBytes("CODB");

        readonly FileStream _fs;
        readonly BinaryReader _br;
        readonly long _offSeeds;

        readonly byte _riceK;
        readonly byte[] _counts;
        readonly uint[] _checkpointSum;
        readonly long[] _checkpointBit;

        public CompressedLightDBSearcher(string path)
        {
            var sw = Stopwatch.StartNew();

            _fs = new FileStream(path, FileMode.Open, FileAccess.Read);
            _br = new BinaryReader(_fs);

            var magic = _br.ReadBytes(4);
            if (!magic.SequenceEqual(MAGIC))
                throw new Exception($"bad magic: {path} はCODB形式ではありません。");

            var version = _br.ReadUInt32();
            if (version != VERSION)
                throw new Exception($"unsupported version {version} (expected {VERSION}): {path}");

            _offSeeds = (long)_br.ReadUInt64();
            _br.ReadUInt64(); // checksum(検索では使わない)
            _riceK = _br.ReadByte(); // +24: Rice符号化パラメータk

            var tableBytes = _offSeeds - HEADER_LEN;
            if (tableBytes < 0)
                throw new Exception("bad seeds offset");
            if (tableBytes % 8 != 0)
                throw new Exception("counts section not 8B-aligned");
            if (_fs.Length < _offSeeds || (_fs.Length - _offSeeds) % 2 != 0)
                throw new Exception("bad seeds length");

            // 個数列を一括ロード(約4MB)。
            _fs.Seek(HEADER_LEN, SeekOrigin.Begin);
            _counts = _br.ReadBytes((int)tableBytes);
            if (_counts.Length != tableBytes) throw new Exception("failed to read counts section");

            // チェックポイント構築: 全P(約800万)を先頭から1回線形にRice復号しながら、
            // 256個ごとに(累積エントリ数, 符号列中のビット位置)を記録する。
            var checkpointCount = (P_COUNT + CHECKPOINT_STRIDE - 1) / CHECKPOINT_STRIDE; // 31,104
            _checkpointSum = new uint[checkpointCount];
            _checkpointBit = new long[checkpointCount];
            long bitPos = 0;
            long sum = 0;
            for (long p = 0; p < P_COUNT; p++)
            {
                if ((p & (CHECKPOINT_STRIDE - 1)) == 0)
                {
                    var idx = p >> 8;
                    _checkpointSum[idx] = (uint)sum;
                    _checkpointBit[idx] = bitPos;
                }
                sum += DecodeOne(ref bitPos);
            }

            sw.Stop();
            Console.WriteLine($"[open] load+build: {sw.Elapsed.TotalMilliseconds:F2}ms");
        }

        /// <summary>
        /// 符号列のビット位置bitPosから1件のRice符号を復号し、個数を返します。
        /// bitPosは復号したぶんだけ前進させます(商qのunary + kbitの剰余、LSBファースト)。
        /// </summary>
        uint DecodeOne(ref long bitPos)
        {
            uint q = 0;
            while (ReadBit(ref bitPos) != 0) q++;

            uint rem = 0;
            for (int i = 0; i < _riceK; i++)
            {
                if (ReadBit(ref bitPos) != 0) rem |= 1u << i;
            }

            var z = (q << _riceK) | rem;
            var r = (int)(z >> 1) ^ -(int)(z & 1); // zigzag逆変換
            return (uint)(COUNT_CENTER + r);
        }

        uint ReadBit(ref long bitPos)
        {
            var byteIdx = bitPos >> 3;
            var shift = (int)(bitPos & 7);
            bitPos++;
            return (uint)((_counts[byteIdx] >> shift) & 1u);
        }

        /// <summary>
        /// プレフィックスPに対応するエントリ範囲[lo, hi)を、チェックポイントからの残り復号で求めます。
        /// checkpointSum[P&gt;&gt;8]・checkpointBit[P&gt;&gt;8]がP&amp;~0xFF(256の倍数)時点の
        /// 累積値・ビット位置なので、そこからPまで復号するだけでよく、走査量は最大256件に収まります。
        /// </summary>
        (uint lo, uint hi) GetRange(long p)
        {
            var checkpointIdx = p >> 8;
            var bitPos = _checkpointBit[checkpointIdx];
            var start = _checkpointSum[checkpointIdx];
            var q0 = checkpointIdx << 8;
            uint cnt = 0;
            for (var q = q0; q <= p; q++)
            {
                cnt = DecodeOne(ref bitPos);
                if (q < p) start += cnt;
            }
            return (start, start + cnt);
        }

        /// <summary>
        /// 観測8回分の(トレーナー名, 自チーム)から起点seed候補を検索します。
        /// 先頭(観測1回目、c0)は起点seed特定には使わず、残り7回(c1..c7)で照合します。
        /// 戻り値はLightDBの意味論と同じ「8回生成後(=c1..c7の7回生成後)のseed」です。
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

            var (lo, hi) = GetRange(p);
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
