from functools import lru_cache
class Key:
 count=0
 def __init__(self,n): self.n=n
 def __hash__(self): return 17
 def __eq__(self,other):
  Key.count+=1
  if Key.count==3: print('nested',f(Key('b')))
  return self.n==other.n
count=0
@lru_cache
def f(x):
 global count
 count+=1
 return count
def current():
 print('a',f(Key('a'))); print('b',f(Key('b'))); print('again',f(Key('b')),count)
 for i in range(127): f(i)
 print('after',f(Key('b')),count)
