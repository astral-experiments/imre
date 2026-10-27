def add(a,b): return a+b
def sub(a,b): return a-b
def mul(a,b): return a*b
def div(a,b): return a/b
def floordiv(a,b): return a//b
def mod(a,b): return a%b
def current():
    for value in (-(1 << 130), True, False, -0.0, float('-inf')):
        magnitude = abs(value)
        print(type(magnitude).__name__, repr(magnitude))
    for call in (lambda: abs(), lambda: abs(1, 2), lambda: abs(x=2)):
        try:
            call()
        except TypeError as error:
            print(error.args)
        else:
            raise AssertionError("invalid abs call accepted")
    for name,func in (('+',add), ('-',sub), ('*',mul), ('/',div), ('//',floordiv), ('%',mod)):
        for a,b in ((1.5,0.5),(1,0.5),(0.5,1),(-7.0,3.0),(7.0,-3.0),(0.0,-3.0),(-0.0,3.0),(1.0,0.0),(1e308,1e-308)):
            try: print(name,repr(func(a,b)))
            except Exception as error: print(name,type(error).__name__,str(error))

    for a,b in ((2.0, 3.0), (-2.0, 3.0), (-4.0, 0.5), (-0.0, 3.0), (0.0, -1.0), (1.0, float("nan")), (1e300, 2.0)):
        try: print("**", repr(a**b))
        except Exception as error: print("**", type(error).__name__, str(error))

    print((-2) ** -3, pow(base=2, exp=-4), pow(2, -3, -13))
    try:
        pow(6, -1, 18)
    except ValueError as error:
        print(str(error))

    for call in (lambda: pow(2, base=3), lambda: pow(2, 3, base=4),
                 lambda: pow(2, 3, modd=4), lambda: pow(2, 3.0, 5)):
        try:
            call()
        except TypeError as error:
            print(error.args)
