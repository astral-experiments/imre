def current():
    print(sum([2+1j, 10E100j, 1j, -10E100j]))
    print(sum([1j, 1, 10E100j, 1j, 1.0, -10E100j]))
    print(sum([2j, 1., 10E100, 1., -10E100]))
    print(sum([2j, 1.0, 10**100, 1.0, -10**100]))
    print(sum([1.0, 10E100, 1.0, -10E100, 1j, 10E100j, 1j, -10E100j]))
    print(sum([complex(1, -0.0), 1.0]), sum([1.0, complex(1, -0.0)]))
    try:
        sum([1j, 10**1000])
    except OverflowError as error:
        print(str(error))
