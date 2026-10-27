def current():
    values = [1, 2, 3]
    rows = []
    for number, label in zip(values, ("a", "b"), strict=True):
        rows.append((number, label))
        values[1] = 7
        if label == "b":
            break
    print(rows, list(zip()), list(zip([1, 2], [3])))
    huge = range(-4611686018427387904, 4611686018427387904)
    for first in huge:
        print(first)
        break
    print(list(zip([3], huge)))

    for lengths in ((2, 1, 2), (1, 1, 2), (2, 2, 2)):
        seen = []
        try:
            for row in zip(range(lengths[0]), range(lengths[1]), range(lengths[2]), strict=True):
                seen.append(row)
        except ValueError as error:
            print(str(error))
        print(seen)

    nested = [42]
    for _ in range(6000):
        nested = zip(nested)
    for row in nested:
        print("nested row")
        break
    print(list(nested))
