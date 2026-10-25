"""Cross-family identity with the native csv and shutil owners' public operations."""
import csv
import shutil
import unittest

case = unittest.TestCase()
reader = csv.reader(['"unterminated'], strict=True)
try:
    next(reader)
except csv.Error as original:
    with case.assertRaises(csv.Error) as captured:
        with case.assertRaises((shutil.Error, OSError)):
            raise original
    print(captured.exception is original)
else:
    raise AssertionError("invalid CSV did not raise")

same = shutil.SameFileError("same")
with case.assertRaises(shutil.Error) as captured_copy:
    with case.assertRaises(csv.Error):
        raise same
print(captured_copy.exception is same)
try:
    with case.assertRaises((csv.Error, shutil.SameFileError)):
        pass
except AssertionError as message:
    print(str(message))
