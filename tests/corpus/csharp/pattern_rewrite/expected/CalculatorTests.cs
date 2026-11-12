using NUnit.Framework;

namespace Calc.Tests
{
    public class CalculatorTests
    {
        [Test]
        public void AddsNumbers()
        {
            var calc = new Calculator();
            Assert.That(calc.Add(2, 2), Is.EqualTo(4));
            Assert.That(calc.Add(-1, 1), Is.EqualTo(0));
        }
    }
}
