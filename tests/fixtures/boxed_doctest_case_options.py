import doctest
import json


case_type = doctest.DocTestCase
calls = []


def make_case(test, *args, **options):
    calls.append((len(args), tuple(options)))
    return case_type(test, *args, **options)


doctest.DocTestCase = make_case
try:
    doctest.DocTestSuite(json)
    doctest.DocTestSuite(json, checker=None, optionflags=0)
finally:
    doctest.DocTestCase = case_type

assert calls == [(0, ()), (0, ("checker", "optionflags"))]
