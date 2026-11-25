// A ledger of accounts: the property suite's C# sample.
using System;
using System.Collections.Generic;
using System.Linq;
using System.Threading.Tasks;

namespace Example.Accounts
{
    public enum Kind
    {
        Asset,
        Liability,
        Income,
        Expense,
    }

    public sealed record Entry(DateTime When, string Account, decimal Amount, string Memo);

    public class LedgerException : Exception
    {
        public LedgerException(string message) : base(message) { }
    }

    public interface IClock
    {
        DateTime Now { get; }
    }

    internal sealed class SystemClock : IClock
    {
        public DateTime Now => DateTime.UtcNow;
    }

    [Serializable]
    public class Ledger
    {
        private readonly Dictionary<string, Kind> _accounts = new();
        private readonly List<Entry> _entries = new List<Entry>();
        private readonly IClock _clock;

        public Ledger(IClock clock = null)
        {
            _clock = clock ?? new SystemClock();
        }

        public int Count => _entries.Count;

        public void Open(string account, Kind kind)
        {
            if (_accounts.ContainsKey(account))
            {
                throw new LedgerException($"account {account} is already open");
            }
            _accounts[account] = kind;
        }

        public void Post(string debit, string credit, decimal amount, string memo = "")
        {
            if (amount <= 0m)
            {
                throw new ArgumentOutOfRangeException(nameof(amount), "must be positive");
            }
            foreach (var account in new[] { debit, credit })
            {
                if (!_accounts.ContainsKey(account))
                {
                    throw new LedgerException($"no account {account}");
                }
            }
            var now = _clock.Now;
            _entries.Add(new Entry(now, debit, amount, memo));
            _entries.Add(new Entry(now, credit, -amount, memo));
        }

        public decimal Balance(string account) =>
            _entries.Where(e => e.Account == account).Sum(e => e.Amount);

        public IReadOnlyDictionary<Kind, decimal> Totals()
        {
            return _accounts
                .GroupBy(pair => pair.Value)
                .ToDictionary(group => group.Key, group => group.Sum(pair => Balance(pair.Key)));
        }

        public IEnumerable<Entry> Between(DateTime from, DateTime to)
        {
            foreach (var entry in _entries)
            {
                if (entry.When >= from && entry.When < to)
                {
                    yield return entry;
                }
            }
        }

        public static string Describe(Kind kind)
        {
            switch (kind)
            {
                case Kind.Asset:
                    return "what we own";
                case Kind.Liability:
                    return "what we owe";
                default:
                    return kind.ToString().ToLowerInvariant();
            }
        }

        public string Label(Kind kind) => kind switch
        {
            Kind.Income => "in",
            Kind.Expense => "out",
            _ => "held",
        };

        public async Task<int> ExportAsync(Func<Entry, Task> write)
        {
            var written = 0;
            foreach (var entry in _entries.OrderBy(e => e.When))
            {
                await write(entry);
                written++;
            }
            return written;
        }

        [Obsolete("use Post")]
        protected internal void Record(Entry entry) => _entries.Add(entry);
    }

    public static class Program
    {
        public static async Task Main(string[] args)
        {
            var ledger = new Ledger();
            ledger.Open("cash", Kind.Asset);
            ledger.Open("sales", Kind.Income);
            ledger.Post("cash", "sales", 120.50m, "first sale");
            Console.WriteLine($"cash: {ledger.Balance("cash")}, entries: {ledger.Count}");
            var count = await ledger.ExportAsync(entry => Task.CompletedTask);
            Console.WriteLine(Ledger.Describe(Kind.Asset) + " " + count);
        }
    }
}
