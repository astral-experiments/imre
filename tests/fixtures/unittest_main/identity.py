import unittest
from identity_helpers import InheritedFixture, identify


class First(InheritedFixture):
    def test_a_body(self):
        self.assertEqual(identify(self), '__main__.First.test_a_body')
        print('body ' + self.id())

    def test_b_skip(self):
        raise AssertionError('body after skipped setup')

    def test_c_error(self):
        raise AssertionError('body after failed setup')


class Second(InheritedFixture):
    def test_after(self):
        self.assertEqual(identify(self), '__main__.Second.test_after')
        print('body ' + self.id())


if __name__ == '__main__':
    print(identify(unittest.TestCase()))
    print(identify(InheritedFixture()))
    other = First()
    print(identify(other))
    other._testMethodName = 'new.name'
    print(identify(other))
    print(identify(First()))
    unittest.main()
