import csv
import sys

lines: list[str] = []
for filename in sys.argv[1:]:
    with open(filename, "rb") as stream:
        lines.append(stream.read().decode("utf-8"))
try:
    for row in csv.reader(lines):
        print("row " + repr(row))
except csv.Error as error:
    print("error " + repr(str(error)))
