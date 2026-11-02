import os
import posix


def current():
    assert os.posix_spawn is posix.posix_spawn
    assert os.posix_spawnp is posix.posix_spawnp
    path = "imre-spawn-actions"
    fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    os.close(fd)
    try:
        actions = [
            (os.POSIX_SPAWN_OPEN, 7, path, os.O_WRONLY | os.O_TRUNC, 0),
            (os.POSIX_SPAWN_DUP2, 7, 1),
            (os.POSIX_SPAWN_CLOSE, 7),
        ]
        for spawn, program in ((os.posix_spawn, "/bin/sh"), (os.posix_spawnp, "sh")):
            args = [program, "-c", 'printf "%s" "$SPAWNED_VALUE"']
            pid = spawn(program, args, {"SPAWNED_VALUE": program}, file_actions=actions)
            assert os.waitpid(pid, 0) == (pid, 0)
            with open(path) as output:
                assert output.read() == program
    finally:
        os.unlink(path)
