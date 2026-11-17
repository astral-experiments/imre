#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <linux/filter.h>
#include <linux/seccomp.h>
#include <stddef.h>
#include <stdlib.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <unistd.h>
__attribute__((constructor)) static void deny_exec(void) {
    struct sock_filter f[] = {
        BPF_STMT(BPF_LD|BPF_W|BPF_ABS, offsetof(struct seccomp_data,nr)),
        BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K, __NR_execve, 2, 0),
        BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K, __NR_execveat, 1, 0),
        BPF_STMT(BPF_RET|BPF_K, SECCOMP_RET_ALLOW),
        BPF_STMT(BPF_RET|BPF_K, SECCOMP_RET_ERRNO|EPERM),
    };
    struct sock_fprog p = {.len=sizeof(f)/sizeof(f[0]), .filter=f};
    const char *path=getenv("DENYEXEC_LOG");
    int fd=path?open(path,O_WRONLY|O_CREAT|O_APPEND|O_CLOEXEC,0600):-1;
    if (fd<0 || prctl(PR_SET_NO_NEW_PRIVS,1,0,0,0) || prctl(PR_SET_SECCOMP,SECCOMP_MODE_FILTER,&p)) {
        if (fd>=0) {write(fd,"filter installation failed\n",27); close(fd);} _exit(125);
    }
    write(fd,"execve and execveat denied with EPERM\n",38); close(fd);
}
