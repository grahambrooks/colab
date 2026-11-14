import unittest

from app.service import place, total


class ServiceTest(unittest.TestCase):
    def test_empty(self):
        order, note = place([])
        self.assertEqual(note, "empty")
        self.assertEqual(order.items, [])

    def test_total(self):
        order, _ = place(["a", "b"])
        self.assertEqual(total([order]), 2)
