# Complete test_io registration on public Imre

Public Imre 915ea527aa75c5787f9d566c0db939f4b459b46f (PR #3960, parent e609d8fe3e0ad7e1b76da9b666fe30d9207594ac) compiled the full unchanged CPython test_io module at boxed O0. The native executable ran the original unittest loader and its list process exited zero. All 667 IDs match pinned CPython in order. The independent standard outputs are byte-identical. Both standard errors are empty.

CPython 3.14.5 source is pinned to 5607950ef232dad16d75c0cf53101d9649d89115. The ELF digest is 7db04e4331ec195b5775f4cb4dbf2923f7088e3981a9d9c019d31bb3ad310b12. This directory records registration. Selected and whole-suite test outcomes have separate receipts.

Run `sha256sum -c SHA256SUMS` and `cmp native/list.stdout cpython/list.stdout` in this directory.
