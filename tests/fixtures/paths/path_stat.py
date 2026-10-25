from pathlib import Path
from os import stat_result


p = Path('input.txt')
st = p.stat()
print(len(st))
print(st.n_fields)
print(st.n_sequence_fields)
print(st.n_unnamed_fields)
print(st.__match_args__)
print(str(st.__match_args__) == repr(st.__match_args__))
print(str(st) == repr(st))
print(st[:] == st)
print(not isinstance(st[:], stat_result))
print(st[-3:] == (st[7], st[8], st[9]))
print(isinstance(st, tuple))
print(isinstance(st, stat_result))
print(isinstance((1, 2), stat_result))
print(st.st_mode == st[0] and st.st_ino == st[1] and st.st_dev == st[2])
print(st.st_nlink == st[3] and st.st_uid == st[4] and st.st_gid == st[5])
print(st.st_size == len(p.read_bytes()) and st.st_size == st[6])
print(st.st_atime_ns // 1_000_000_000 == st[7])
print(st.st_mtime_ns // 1_000_000_000 == st[8])
print(st.st_ctime_ns // 1_000_000_000 == st[9])
print(st.st_atime == st[7] + 1e-9 * (st.st_atime_ns % 1_000_000_000))
print(st.st_mtime == st[8] + 1e-9 * (st.st_mtime_ns % 1_000_000_000))
print(st.st_ctime == st[9] + 1e-9 * (st.st_ctime_ns % 1_000_000_000))
print(st.st_blksize > 0 and st.st_blocks >= 0 and st.st_rdev == 0)
clock = Path('stat-clock').stat()
print(clock[7])
print(clock.st_atime_ns)
print(clock.st_atime == clock[7] + 1e-9 * (clock.st_atime_ns % 1_000_000_000))
print(clock[8])
print(clock.st_mtime_ns)
print(clock.st_mtime == clock[8] + 1e-9 * (clock.st_mtime_ns % 1_000_000_000))
try:
    print(st.st_birthtime > 0 and st.st_birthtime == p.lstat().st_birthtime)
except AttributeError as error:
    print(str(error))
try:
    print(st.st_flags >= 0 and st.st_flags == p.lstat().st_flags)
except AttributeError as error:
    print(str(error))
try:
    print(st.st_gen >= 0 and st.st_gen == p.lstat().st_gen)
except AttributeError as error:
    print(str(error))
try:
    print(st.st_birthtime_ns)
except AttributeError as error:
    print(str(error))
print(repr(st).startswith('os.stat_result(st_mode='))
print(st == (st[0], st[1], st[2], st[3], st[4], st[5], st[6], st[7], st[8], st[9]))
link = Path('file-link')
print(link.stat() == p.lstat())
print(link.lstat() == link.stat(follow_symlinks=False))
print(link.stat() != link.lstat())
print(Path('dangling-link').lstat().st_size == len('missing'))

try:
    Path('dangling-link').stat()
except FileNotFoundError as error:
    print(repr(error.errno))
    print(repr(error.filename))
    print(repr(error.filename2))
try:
    Path('bad\0name').stat(follow_symlinks=False)
except ValueError as error:
    print(str(error))
try:
    Path('bad\0name').lstat()
except ValueError as error:
    print(str(error))
