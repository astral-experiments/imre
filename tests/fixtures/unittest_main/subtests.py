from unittest import TestCase, main


class Subtests(TestCase):
    def test_nested(self) -> None:
        args = ["before"]
        with self.subTest("outer", group=1):
            with self.subTest(args=args) as context:
                self.assertIsNone(context)
                args.append("after")
                print("before failure")
                self.fail("first")
            print("continued")
            with self.subTest(group=2):
                raise ValueError("second")
            with self.subTest(reason=None):
                self.skipTest("third")
        print("after outer")

    def test_next(self) -> None:
        scope = self.subTest()
        with scope:
            print("next case")
        with self.assertRaises(AttributeError):
            with scope:
                self.fail("entered a consumed context")

    def tearDown(self) -> None:
        print("teardown")


if __name__ == "__main__":
    try:
        with TestCase().subTest():
            raise ValueError("unattached")
    except ValueError:
        print("propagated")
    main()
