# Three unchanged deque methods under their original host fixtures

On the exact product tree for [#1122](https://github.com/astral-sh/imre/pull/1122), clear, maxlen zero, and init run native bodies and assertions under host fixtures. The original assertions detect a disabled clear, a zero-capacity constructor that fails to consume its iterator, and the independent suppression of either constructor error. All four controls fail the original assertions at both levels.

For each of 659 occurrences at O0 and O3 the report retains the previously verified observation on these binaries, except for the three independently audited deque operations. Its counts are 15 native bodies under host fixtures, 14 host-driven native operations, four with no collection operation, three reference skips and 623 blocked. No native-fixture result from another revision is imported.
