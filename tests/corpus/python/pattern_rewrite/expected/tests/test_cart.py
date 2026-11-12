import pdb
import unittest

from shop.cart import Cart


class CartTest(unittest.TestCase):
    def test_total(self):
        cart = Cart()
        cart.add("apple", 2)
        self.assertEqual(cart.count(), 2)
        self.assertEqual(cart.total(), cart.price("apple") * 2)
