package com.example;

import org.junit.Assert;
import org.junit.Test;

public class CalculatorTest {
    @Test
    public void addsNumbers() {
        Calculator calc = new Calculator();
        assertThat(calc.add(2, 2)).isEqualTo(4);
        assertThat(calc.add(-1, 1)).isEqualTo(0);
    }
}
