import doctest
import itertools
import json

# test_json.load_tests iterates over a tuple of module objects.
for mod in (json, json.encoder, json.decoder, itertools):
    suite = doctest.DocTestSuite(mod)
    print(suite.countTestCases())
    for case in suite:
        print(case.id())
        for example in case._dt_test.examples:
            print(repr(example.source))
            print(repr(example.want))
            print(repr(example.exc_msg))
            print(repr(example.options))
            print(example.lineno)
            print(example.indent)
