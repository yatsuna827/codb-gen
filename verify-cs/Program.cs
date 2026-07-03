using System;
using PokemonCoRNGLibrary;

// codb-gen selftest との突き合わせ用ハーネス。
// 使い方: dotnet run -c Release -- 256 > cs_out.txt
//         codb-gen selftest 256 > rs_out.txt
//         両者が一致すればバトル生成コアはC#ライブラリと同一。
class Program
{
    static void Main(string[] args)
    {
        var n = args.Length > 0 ? uint.Parse(args[0]) : 64u;
        for (uint i = 0; i < n; i++)
        {
            var seed = unchecked(i * 0x9E3779B9u);
            var code = BattleNow.SingleBattle.Ultimate.GenerateCode(seed, out var fin);
            Console.WriteLine($"{seed:X8} {code:D2} {fin:X8}");
        }
    }
}
