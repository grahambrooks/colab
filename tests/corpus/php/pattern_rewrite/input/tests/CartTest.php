<?php

namespace App\Tests;

use PHPUnit\Framework\TestCase;

class CartTest extends TestCase
{
    public function testTotal(): void
    {
        $cart = new Cart();
        $cart->add('apple', 2);
        var_dump($cart);
        $this->assertEquals(2, $cart->count());
        $this->assertEquals('apple', $cart->first()->name);
    }
}
