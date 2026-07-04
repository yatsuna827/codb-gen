using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using PokemonCoRNGLibrary;
using PokemonCOSeedDataBaseAPI;

// codb-gen との突き合わせ用ハーネス。
//
// selftest モード (デフォルト):
//   dotnet run -c Release -- 256 > cs_out.txt
//   codb-gen selftest 256 > rs_out.txt
//   両者が一致すればバトル生成コアはC#ライブラリと同一。
//
// search モード (テスト2: 実物Searcherで生成済みDBから既知seedを引く):
//   dotnet run -c Release -- search <DBディレクトリ> full|light <hexseed>...
//   各seedから GenerateCode で観測列を作り、FullDBSearcher / LightDBSearcher に渡して
//   期待seed(full=7バトル進行後, light=8バトル進行後)が返るか確認する。
//
// csearch モード(圧縮LightDB、CUML/CNTS両形式対応の動作確認):
//   dotnet run -c Release -- csearch <cldbfile> <hexseed>...
//   各seedから8回分の観測コードを作り、CompressedLightDBSearcherに渡して
//   期待seed(8回生成後)が返るか確認する。CUML/CNTSはファイルヘッダのformatタグから
//   自動判別される。
//
// bench モード(圧縮LightDB、CUML/CNTS両形式対応の性能測定):
//   dotnet run -c Release -- bench <cldbfile> <N>
//   N個のランダムseed(シード固定で再現可能)について同様の検索を行い、
//   クエリ1件ごとの所要時間をStopwatchで計測してavg/median/max(ms)を出力する。
class Program
{
    static void Main(string[] args)
    {
        if (args.Length > 0 && args[0] == "search")
        {
            RunSearch(args);
            return;
        }

        if (args.Length > 0 && args[0] == "csearch")
        {
            RunCSearch(args);
            return;
        }

        if (args.Length > 0 && args[0] == "bench")
        {
            RunBench(args);
            return;
        }

        var n = args.Length > 0 ? uint.Parse(args[0]) : 64u;
        for (uint i = 0; i < n; i++)
        {
            var seed = unchecked(i * 0x9E3779B9u);
            var code = BattleNow.SingleBattle.Ultimate.GenerateCode(seed, out var fin);
            Console.WriteLine($"{seed:X8} {code:D2} {fin:X8}");
        }
    }

    static void RunSearch(string[] args)
    {
        var dir = args[1];
        var mode = args[2];
        bool light = mode == "light";
        var searcher = light
            ? SeedSearcher.CreateLightDBSearcher(dir)
            : SeedSearcher.CreateFullDBSearcher(dir);
        int nBattles = light ? 8 : 7;

        int miss = 0;
        foreach (var arg in args.Skip(3))
        {
            var s0 = Convert.ToUInt32(arg, 16);
            var s = s0;
            var keys = new (PlayerName, BattleTeam)[nBattles];
            for (int k = 0; k < nBattles; k++)
            {
                var code = BattleNow.SingleBattle.Ultimate.GenerateCode(s, out s);
                keys[k] = ((PlayerName)(code / 8), (BattleTeam)(code % 8));
            }
            var expected = s;

            var results = searcher.Search(keys).ToArray();
            bool hit = results.Contains(expected);
            if (!hit) miss++;
            Console.WriteLine(
                $"{s0:X8} expect={expected:X8} got=[{string.Join(",", results.Select(r => r.ToString("X8")))}] {(hit ? "HIT" : "MISS")}");
        }
        Console.WriteLine(miss == 0 ? "ALL HIT" : $"{miss} MISS");
        Environment.Exit(miss == 0 ? 0 : 1);
    }

    // 8回分の観測コードをseedから生成する(GenerateCodeの連鎖)。
    static (PlayerName, BattleTeam)[] GenerateKeys(ref uint s)
    {
        var keys = new (PlayerName, BattleTeam)[8];
        for (int k = 0; k < 8; k++)
        {
            var code = BattleNow.SingleBattle.Ultimate.GenerateCode(s, out s);
            keys[k] = ((PlayerName)(code / 8), (BattleTeam)(code % 8));
        }
        return keys;
    }

    static void RunCSearch(string[] args)
    {
        var v2Path = args[1];
        using var searcher = new CompressedLightDBSearcher(v2Path);

        int miss = 0;
        foreach (var arg in args.Skip(2))
        {
            var s0 = Convert.ToUInt32(arg, 16);
            var s = s0;
            var keys = GenerateKeys(ref s);
            var expected = s;

            var results = searcher.Search(keys).ToArray();
            bool hit = results.Contains(expected);
            if (!hit) miss++;
            Console.WriteLine(
                $"{s0:X8} expect={expected:X8} got=[{string.Join(",", results.Select(r => r.ToString("X8")))}] {(hit ? "HIT" : "MISS")}");
        }
        Console.WriteLine(miss == 0 ? "ALL HIT" : $"{miss} MISS");
        Environment.Exit(miss == 0 ? 0 : 1);
    }

    static void RunBench(string[] args)
    {
        var v2Path = args[1];
        var count = int.Parse(args[2]);

        using var searcher = new CompressedLightDBSearcher(v2Path);

        var rnd = new Random(12345); // 再現性のため固定シード
        var times = new List<double>(count);
        int miss = 0;
        double firstMs = 0;
        bool firstHit = false;

        for (int i = 0; i < count; i++)
        {
            var buf = new byte[4];
            rnd.NextBytes(buf);
            var s0 = BitConverter.ToUInt32(buf, 0);

            var s = s0;
            var keys = GenerateKeys(ref s);
            var expected = s;

            var sw = Stopwatch.StartNew();
            var results = searcher.Search(keys).ToArray();
            sw.Stop();

            bool hit = results.Contains(expected);
            if (!hit) miss++;

            if (i == 0)
            {
                firstMs = sw.Elapsed.TotalMilliseconds;
                firstHit = hit;
            }
            else
            {
                times.Add(sw.Elapsed.TotalMilliseconds);
            }
        }

        Console.WriteLine($"[first query(ファイルキャッシュ影響あり)] {firstMs:F2}ms {(firstHit ? "HIT" : "MISS")}");

        times.Sort();
        double avg = times.Count > 0 ? times.Average() : 0;
        double median = times.Count > 0 ? times[times.Count / 2] : 0;
        double max = times.Count > 0 ? times[times.Count - 1] : 0;

        Console.WriteLine(miss == 0 ? "ALL HIT" : $"{miss} MISS");
        Console.WriteLine(
            $"queries={count} (先頭1件を除く{times.Count}件で集計) avg={avg:F2}ms median={median:F2}ms max={max:F2}ms");
        Environment.Exit(miss == 0 ? 0 : 1);
    }
}
