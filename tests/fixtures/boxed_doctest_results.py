from doctest import TestResults


result = TestResults(1, 3, skipped=2)
assert isinstance(result, tuple)
assert (result.failed, result.attempted) == tuple(result) == (1, 3)
assert result == TestResults(1, 3)
assert result.skipped == 2
assert repr(result) == "TestResults(failed=1, attempted=3, skipped=2)"
result.skipped = 0
assert repr(result) == "TestResults(failed=1, attempted=3)"
