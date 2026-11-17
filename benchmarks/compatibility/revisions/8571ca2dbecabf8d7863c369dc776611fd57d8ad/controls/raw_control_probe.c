// Small traced supervisor for gate and saved-image source withdrawal controls.
#include <errno.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>
#include <stdio.h>
#include <string.h>

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
        char *args[] = {argv[5], NULL};
        int code = before ? child(args) : -1;
        int after = absent(tree) && absent(leaf);
        int restored = !rename(stash, tree) && !stat(leaf, &st);
        if (!before || !after || !restored || code < 0) return 73;
        return code;
    }
    return 64;
}
