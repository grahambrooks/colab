import unittest


class TestMath(unittest.TestCase):
    def test_add(self):
        self.assertEquals(1 + 1, 2)
        self.assertEquals(add(2, 3), 5)

    def test_truth(self):
        self.assertTrue(1 == 1)
        self.assertTrue(add(1, 1) == 2)
