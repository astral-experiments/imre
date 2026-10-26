from unittest import TestCase


class InheritedFixture(TestCase):
    def setUp(self):
        name = self.id().split('.')[-1]
        self.assertEqual(name, self._testMethodName)
        print('setup ' + self.id())
        if name == 'test_b_skip':
            self.skipTest('setup skipped ' + self.id())
        if name == 'test_c_error':
            raise ValueError('setup failed ' + self.id())

    def tearDown(self):
        print('teardown ' + self.id())


def identify(case: TestCase) -> str:
    return case.id()
