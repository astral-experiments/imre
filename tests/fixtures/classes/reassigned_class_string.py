class C:
    label = "fixed"


class Child(C):
    pass


parent = C()
child = Child()
C.label = "new"
print(C.label, parent.label, Child.label, child.label)
Child.label = "child"
print(C.label, parent.label, Child.label, child.label)
