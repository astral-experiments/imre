from collections import deque
class Mutator:
 def __init__(self,answer): self.answer=answer
 def __eq__(self,other): outer.append(99); print('eq'); return self.answer
 def __ne__(self,other): print('ne'); return True
def current():
 global outer
 for answer in (True,False):
  for inequality in (False,True):
   outer=deque([deque([Mutator(answer)])]); other=deque([deque([Mutator(answer)])])
   try: print(answer,inequality,outer!=other if inequality else outer==other)
   except Exception as e: print(type(e).__name__,str(e))
   print(len(outer))
