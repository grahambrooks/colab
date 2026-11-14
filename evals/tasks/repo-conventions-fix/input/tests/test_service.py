import unittest

from app.service import place, total


class ServiceTest(unittest.TestCase):
    def test_empty(self):
        order, note = place([])
        self.assertEquals(note, "empty")
        self.assertEquals(order.items, [])

    def test_total(self):
        order, _ = place(["a", "b"])
        self.assertEquals(total([order]), 2)
