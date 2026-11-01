import csv


def main() -> None:
    try:
        next(csv.reader(['"ab"c'], strict=1))
    except csv.Error as error:
        print(str(error))
    print(next(csv.reader(['"ab"c'], doublequote=0)))
    print(next(csv.reader(['a, b'], skipinitialspace=-4)))
    print(next(csv.reader(['a, b'], skipinitialspace=+0.0)))
    print(next(csv.reader(['a, b'], skipinitialspace="yes")))
    print(next(csv.reader(['a, b'], skipinitialspace=b"")))
    print(next(csv.reader(['a, b'], skipinitialspace=None)))


main()
