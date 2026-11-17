#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <linux/audit.h>
#include <linux/filter.h>
#include <linux/seccomp.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <unistd.h>

static void marker(const char *s) { write(STDERR_FILENO, s, strlen(s)); }
__attribute__((constructor)) static void guard(void) {
    FILE *f = fopen("/proc/self/maps", "r"); char line[4096];
    if (!f) { marker("[guard: maps unavailable]\n"); _exit(125); }
    while (fgets(line, sizeof line, f)) if (strcasestr(line, "libpython")) {
        marker("[guard: libpython already mapped]\n"); _exit(125);
    }
    fclose(f);
    struct sock_filter filter[] = {
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, arch)),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, AUDIT_ARCH_X86_64, 1, 0),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM),
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, nr)),
        BPF_JUMP(BPF_JMP | BPF_JSET | BPF_K, 0x40000000, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_execve, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_execveat, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    struct sock_fprog prog = { .len = sizeof filter / sizeof filter[0], .filter = filter };
    if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) || prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog)) {
        marker("[guard: seccomp unavailable]\n"); _exit(125);
    }
    marker("[guard: execve/execveat denied; no libpython mapped]\n");
}
void *dlopen(const char *name, int flags) {
    if (name && strcasestr(name, "libpython")) { marker("[guard: libpython dlopen denied]\n"); errno = EPERM; return NULL; }
    static void *(*next)(const char *, int);
    if (!next) next = dlsym(RTLD_NEXT, "dlopen");
    return next(name, flags);
}
