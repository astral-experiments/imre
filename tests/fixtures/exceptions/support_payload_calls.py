def seen(label: str) -> str:
    print(label)
    return label


def exercise() -> None:
    plain = TestFailed('base', 3, 'more', stats='counts')
    print(plain.msg)
    print(plain.stats)
    print(str(plain))
    print(repr(plain))
    print(repr(plain.args))
    child = TestFailedWithDetails('message', ('BadCase', 11), ('FailCase', 4), 'stats')
    print(child.errors)
    print(child.failures)
    print(child.stats)
    print(str(child))
    print(repr(child))
    print(repr(child.args))
    try:
        raise child
    except TestDidNotRun:
        print('wrong sibling')
    except TestFailed as caught:
        print(caught is child)
        print(caught.msg)
        print(caught.stats)
        print(repr(caught.args))
    try:
        raise child
    except Error as caught:
        print(str(caught))
        print(repr(caught))

exercise()

errors: list[tuple[str,str]] = [('source', 'first')]
failures: list[tuple[str,str]] = []
stats: list[str] = ['started']
problem = TestFailedWithDetails('msg', errors, failures, stats)
print(repr(problem.errors))
print(repr(problem.args))
errors.append(('second', 'detail'))
stats.append('finished')
print(repr(problem.errors))
print(repr(problem.stats))
print(repr(problem.args))
print(repr(problem))

try:
    print(str(TestFailed(17)))
except TypeError as caught:
    print(str(caught))
try:
    print(str(TestFailed(None)))
except TypeError as caught:
    print(str(caught))

try:
    TestFailed()
except TypeError as caught:
    print(str(caught))
try:
    TestFailed(seen('first'), msg=seen('second'))
except TypeError as caught:
    print(str(caught))
try:
    TestFailedWithDetails(seen('message'), seen('error'), seen('fail'), seen('stats'), seen('extra'))
except TypeError as caught:
    print(str(caught))
try:
    TestFailedWithDetails('msg', 'errors', 'failures')
except TypeError as caught:
    print(str(caught))
try:
    TestFailed('msg', wrong=seen('bad kw'))
except TypeError as caught:
    print(str(caught))

print(len(problem.errors))
print(repr(problem.errors[-1]))
print(repr(problem.errors[-1][-1]))
print(problem.errors is errors)
print(problem.errors is not failures)
print(problem.stats == stats)
print(TestFailed("default").stats is None)
print(bool(problem.errors))
print(bool(problem.failures))
try:
    print(problem.errors[20])
except IndexError as caught:
    print(str(caught))
try:
    print(str(TestFailed(errors)))
except TypeError as caught:
    print(str(caught))
try:
    print(str(TestFailed(1.5)))
except TypeError as caught:
    print(str(caught))
print(TestFailed(1.0).msg == True)
print(TestFailed(2**80).msg == float(2**80))
print(TestFailed([True]).msg == [1])
print(TestFailed((1, [False])).msg == (True, [0]))
wrapped = TestFailed(problem.errors, problem.msg)
print(repr(wrapped.args))
print(wrapped.msg is errors)
try:
    TestFailed("message", stat=seen("mistyped stats"))
except TypeError as caught:
    print(str(caught))
try:
    TestFailed("message", self=seen("duplicate self"))
except TypeError as caught:
    print(str(caught))
