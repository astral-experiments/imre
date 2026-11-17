#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>
int main(int argc,char **argv) {
    if (argc!=2) return 2;
    if (!strcmp(argv[1],"exec")) {
        pid_t p=fork(); if(p<0)return 3;
        if(!p){execl("/bin/true","true",(char *)0);_exit(errno==EPERM?42:1);}
        int status=0; if(waitpid(p,&status,0)!=p)return 4;
        if(WIFEXITED(status)&&WEXITSTATUS(status)==42){puts("child execve denied: EPERM");return 0;}return 1;
    }
    if (!strcmp(argv[1],"python")) {dlopen("libpython3.13.so.1.0",RTLD_NOW);return 1;}
    return 2;
}
