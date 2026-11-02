import sys
from unittest import TestCase


class Subject:
    present = None

    @property
    def broken(self):
        raise RuntimeError("lookup failed")


def current():
    case = TestCase()
    subject = Subject()
    assert case.assertNotHasAttr(subject, "absent") is None
    for target, name, expected in (
        (subject, "present", "'Subject' object has unexpected attribute 'present'"),
        (Subject, "present", "type object 'Subject' has unexpected attribute 'present'"),
        (sys, "modules", "module 'sys' has unexpected attribute 'modules'"),
    ):
        with case.assertRaises(AssertionError) as caught:
            case.assertNotHasAttr(target, name, msg="detail")
        assert str(caught.exception) == expected + " : detail"
    with case.assertRaises(RuntimeError) as caught:
        case.assertNotHasAttr(subject, "broken")
    assert str(caught.exception) == "lookup failed"

    assert case.assertHasAttr(subject, "present") is None
    for target, expected in (
        (subject, "'Subject' object has no attribute 'absent'"),
        (Subject, "type object 'Subject' has no attribute 'absent'"),
        (sys, "module 'sys' has no attribute 'absent'"),
    ):
        with case.assertRaises(AssertionError) as caught:
            case.assertHasAttr(target, "absent", msg="detail")
        assert str(caught.exception) == expected + " : detail"
    with case.assertRaises(RuntimeError) as caught:
        case.assertHasAttr(subject, "broken")
    assert str(caught.exception) == "lookup failed"
