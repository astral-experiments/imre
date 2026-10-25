"""Exercise both handler orders on a build containing the native CSV and shutil modules."""

import csv
import shutil
import sys
from pathlib import Path

reader = csv.reader(['"unterminated'], strict=True)
try:
    next(reader)
except shutil.Error:
    raise AssertionError("CSV error caught by shutil.Error")
except OSError:
    raise AssertionError("CSV error caught by OSError")
except csv.Error:
    print("csv")

try:
    shutil.copyfile(sys.argv[1], sys.argv[1])
except csv.Error:
    raise AssertionError("SameFileError caught by csv.Error")
except shutil.SameFileError:
    print("same file")

try:
    shutil.copyfile(sys.argv[1], sys.argv[1])
except csv.Error:
    raise AssertionError("SameFileError caught by csv.Error")
except shutil.Error:
    print("shutil base")

try:
    shutil.copyfile(sys.argv[2], sys.argv[1])
except csv.Error:
    raise AssertionError("SpecialFileError caught by csv.Error")
except shutil.Error:
    raise AssertionError("SpecialFileError caught by shutil.Error")
except shutil.SpecialFileError:
    print("special file")

try:
    raise shutil.Error("copy")
except csv.Error:
    raise AssertionError("shutil.Error caught by csv.Error")
except OSError:
    print("OS base")


reader = csv.reader(['"unterminated'], strict=True)
try:
    next(reader)
except UnicodeEncodeError:
    raise AssertionError("CSV error caught by UnicodeEncodeError")
except csv.Error:
    print("CSV versus codec")

try:
    Path("encoding.txt").write_text("é", encoding="ascii")
except csv.Error:
    raise AssertionError("codec error caught by csv.Error")
except UnicodeEncodeError:
    print("codec versus CSV")
