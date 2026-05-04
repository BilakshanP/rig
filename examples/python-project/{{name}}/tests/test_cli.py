import unittest


class TestCli(unittest.TestCase):
    def test_import(self) -> None:
        import {{package}}.cli
        self.assertTrue(hasattr({{package}}.cli, "main"))


if __name__ == "__main__":
    unittest.main()
