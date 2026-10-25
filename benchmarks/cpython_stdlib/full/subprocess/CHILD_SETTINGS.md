# Native child settings

At O0 and O3, [the paired replay](child-settings-linux.json) uses #693 at
`7c05412` (tree `baa4ae2ea7937acd5c4ba129e6f6addf533c2651`). Native Popen
sets the session or process group before exec; the original CPython data child
reports its own PID from getsid or getpgid. With umask 0053 it creates a file
with mode 0624. The native parent reaps it once in all three cases.

The unchanged bodies, fixtures and assertions execute on CPython. The shared
runner's native-body diagnostic remains attached to each ID. These are three
host-driven native operations in the 353-occurrence report. The
[earlier hosted run](child-settings-hosted-linux.json) keeps its artifact and
checksums separately.

Run `child_settings_originals.py --imre ... --runtime ... --clang ...
--opt-level {0,3} --result ...` with the pinned interpreter and strace.
Use `child_settings_evidence.py --previous PREVIOUS --child-settings
child-settings-linux.json --result RESULT` to apply just these three records.
