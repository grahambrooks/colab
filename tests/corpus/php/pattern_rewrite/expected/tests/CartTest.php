<?php

namespace App\Tests;

use PHPUnit\Framework\TestCase;

class CartTest extends TestCase
{
    public function testTotal(): void
    {
        $cart = new Cart();
        $cart->add('apple', 2);
        $this->assertSame(2, $cart->count());
        $this->assertSame('apple', $cart->first()->name);
    }
}
