import operator

def current():
    e=enumerate([1,2])
    print('type',type(e) is enumerate,isinstance(e,enumerate),callable(enumerate))
    for obj in (enumerate,type(e)):
        for name in ('__name__','__qualname__','__module__'):
            try: print(name,getattr(obj,name))
            except Exception as error: print(name,type(error).__name__)
    print('hint',operator.length_hint(e,37))
    print('next',next(e))
