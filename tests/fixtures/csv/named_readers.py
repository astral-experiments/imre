import csv


def main() -> None:
    print(repr(list(csv.reader(['"a,b",c'], 'unix'))))
    print(repr(list(csv.reader(['a\tb,c'], dialect='excel-tab'))))
    print(repr(list(csv.reader(['a,b\tc'], delimiter=',', dialect='excel-tab'))))
    print(repr(list(csv.reader(['a|b,c'], 'excel', delimiter='|'))))
    print(repr(list(csv.reader(['"a"'], None, quotechar=None))))
    try:
        csv.reader([], 'unknown', lineterminator=None)
    except csv.Error as error:
        print(str(error))
    try:
        csv.reader([], 'excel', quotechar=None)
    except TypeError as error:
        print(str(error))
    with open("named.csv", "w") as source:
        source.write("a,b")
    try:
        csv.reader(source, 'unknown')
    except ValueError as error:
        print(str(error))


main()
