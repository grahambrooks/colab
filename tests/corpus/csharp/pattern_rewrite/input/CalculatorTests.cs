using NUnit.Framework;

namespace Calc.Tests
{
    public class CalculatorTests
    {
        [Test]
        public void AddsNumbers()
        {
            var calc = new Calculator();
            Console.WriteLine("calc ready");
            Assert.AreEqual(4, calc.Add(2, 2));
            Assert.AreEqual(0, calc.Add(-1, 1));
        }
    }
}
