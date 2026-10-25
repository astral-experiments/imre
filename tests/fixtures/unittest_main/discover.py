import unittest as ut
from support_cases import ImportedCase as Zed


class Utility:
    def test_not_a_case(self) -> None:
        raise AssertionError("non-TestCase classes must not be collected")


class Base(ut.TestCase):
    def setUp(self) -> None:
        print("setup")

    def tearDown(self) -> None:
        print("teardown")

    def test_b(self) -> None:
        print("inherited")


class Child(Base):
    def test_a(self) -> None:
        """The first line is the verbose unittest description.

        Later lines are not part of the test label.
        """
        self.assertEqual(3, 3)


if __name__ == "__main__":
    ut.main()
