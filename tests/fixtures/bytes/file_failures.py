# ruff: noqa: SIM115
# Explicit operations distinguish flush failures, close state, and final cleanup.

full = open("/dev/full", "wb")
alias = full
print(full.write(b"\0\xffbuffered"))
try:
    full.flush()
except OSError:
    print("flush failed")
try:
    full.close()
except OSError:
    print("close failed")
print(alias.closed)
alias.close()

outer = open("outer-cleanup.bin", "wb")
full = open("/dev/full", "wb")
try:
    with outer, full:
        outer.write(b"saved\0\xff")
        full.write(b"buffered")
        raise ValueError("body error")
except OSError:
    print("close replaces body error")
print(outer.closed)
print(full.closed)

try:
    with open("/dev/full", "wb") as large:
        large.write(b"\0\xff" * 1000000)
except OSError:
    print("large write failed")
