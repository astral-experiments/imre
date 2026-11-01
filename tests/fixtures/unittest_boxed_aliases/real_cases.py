import unittest as ut

class Base(ut.TestCase):
    def test_inherited_skip(self):
        print('UNEXPECTED inherited skipped body')

class Active(Base):
    def setUp(self):
        print('SETUP', self.id())

    def tearDown(self):
        print('TEARDOWN', self.id())

    def test_fail(self):
        print('BODY', self.id())
        self.fail('intentional duplicate failure')

    def test_own_skip(self):
        print('UNEXPECTED own skipped body')

    def test_runtime_skip(self):
        print('BODY', self.id())
        self.skipTest('runtime skip reason')

    def test_pass(self):
        print('BODY', self.id())

class SkippedBase(ut.TestCase):
    def test_class_skip(self):
        print('UNEXPECTED inherited class skipped body')

class InheritedSkip(SkippedBase):
    pass

class OwnSkip(ut.TestCase):
    def test_class_skip(self):
        print('UNEXPECTED own class skipped body')

Base.test_inherited_skip.__unittest_skip__ = True
Base.test_inherited_skip.__unittest_skip_why__ = 'inherited method reason'
Active.test_own_skip.__unittest_skip__ = True
Active.test_own_skip.__unittest_skip_why__ = 'own method reason'
SkippedBase.__unittest_skip__ = True
SkippedBase.__unittest_skip_why__ = 'inherited class reason'
OwnSkip.__unittest_skip__ = True
OwnSkip.__unittest_skip_why__ = 'own class reason'
