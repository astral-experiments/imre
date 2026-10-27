def add(a,b): return a+b
def sub(a,b): return a-b
def mul(a,b): return a*b
def div(a,b): return a/b
def floordiv(a,b): return a//b
def mod(a,b): return a%b
def current():
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
