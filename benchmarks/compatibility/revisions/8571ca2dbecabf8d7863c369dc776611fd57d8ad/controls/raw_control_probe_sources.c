// Small traced supervisor for gate and saved-image source withdrawal controls.
#include <errno.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <limits.h>

static int absent(const char *path) {
    struct stat st;
    errno = 0;
    return stat(path, &st) == -1 && errno == ENOENT;
}
static int child(char **argv) {
    pid_t pid = fork(); int status;
    if (pid < 0) return -1;
    if (!pid) { execv(argv[0], argv); _exit(127); }
    if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status)) return -1;
    return WEXITSTATUS(status);
}
static int withdraw_extra(const char *path, char *stash) {
    struct stat st;
    if (!path || snprintf(stash, PATH_MAX, "%s.raw-control-stash", path) >= PATH_MAX) return 0;
    return !stat(path, &st) && absent(stash) && !rename(path, stash) && absent(path);
}
static int restore_extra(const char *path, const char *stash) {
    struct stat st;
    return absent(path) && !rename(stash, path) && !stat(path, &st);
}
int main(int argc, char **argv) {
    if (argc >= 5 && !strcmp(argv[1], "gate")) {
        if (!absent(argv[2])) return 70;
        int code = child(argv + 3);
        if (code <= 0 || !absent(argv[2])) return 71;
        return 0;
    }
    if (argc == 6 && !strcmp(argv[1], "withdraw")) {
        const char *tree = argv[2], *stash = argv[3], *leaf = argv[4];
        struct stat st;
        if (stat(leaf, &st) || !absent(stash) || rename(tree, stash)) return 72;
        int before = absent(tree) && absent(leaf);
        const char *launcher = getenv("IMRE_WITHDRAW_LAUNCHER");
        const char *archive = getenv("IMRE_WITHDRAW_ARCHIVE");
        char launcher_stash[PATH_MAX], archive_stash[PATH_MAX];
        int launcher_hidden = before && withdraw_extra(launcher, launcher_stash);
        int archive_hidden = launcher_hidden && withdraw_extra(archive, archive_stash);
        char *args[] = {argv[5], NULL};
        int code = archive_hidden ? child(args) : -1;
        int after = absent(tree) && absent(leaf);
        int archive_restored = !archive_hidden || restore_extra(archive, archive_stash);
        int launcher_restored = !launcher_hidden || restore_extra(launcher, launcher_stash);
        int restored = archive_restored && launcher_restored && !rename(stash, tree) && !stat(leaf, &st);
        if (!before || !after || !restored || code < 0) return 73;
        return code;
    }
    return 64;
}
