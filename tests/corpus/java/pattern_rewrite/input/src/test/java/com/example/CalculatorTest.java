package com.example;

import org.junit.Assert;
import org.junit.Test;

public class CalculatorTest {
    @Test
    public void addsNumbers() {
        Calculator calc = new Calculator();
        System.out.println("calc: " + calc);
        Assert.assertEquals(4, calc.add(2, 2));
        Assert.assertEquals(0, calc.add(-1, 1));
    }
}
