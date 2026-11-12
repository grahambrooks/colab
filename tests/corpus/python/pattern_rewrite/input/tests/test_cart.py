import pdb
import unittest

from shop.cart import Cart


class CartTest(unittest.TestCase):
    def test_total(self):
        cart = Cart()
        cart.add("apple", 2)
        pdb.set_trace()
        self.assertEquals(cart.count(), 2)
        self.assertEquals(cart.total(), cart.price("apple") * 2)
