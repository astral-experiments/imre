from functools import lru_cache
class Key:
 count=0
 def __init__(self,n): self.n=n
 def __hash__(self): return 17
 def __eq__(self,other):
  Key.count+=1
  if Key.count==3:
   for i in range(130): f(i)
  return self.n==other.n
counter=0
@lru_cache
def f(x):
 global counter
 counter+=1
 return counter
def current():
 f(Key('a')); f(Key('b')); f(130); print(f(2),f(3),f(129),f(130), counter)
