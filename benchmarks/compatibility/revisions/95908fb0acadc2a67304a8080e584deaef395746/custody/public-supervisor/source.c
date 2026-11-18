#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>
#include <string.h>
int main(int argc,char **argv) {
 if(argc<7) return 125;
 const char *source=argv[1],*stash=argv[2],*zip=argv[3],*zipstash=argv[4],*image=argv[5]; struct stat st;
 umask(0077);
 if(lstat(source,&st) || !S_ISREG(st.st_mode) || !lstat(stash,&st) || errno!=ENOENT || rename(source,stash)) return 125;
 int z=strcmp(zip,"-")!=0;
 if(z && (lstat(zip,&st) || !S_ISREG(st.st_mode) || !lstat(zipstash,&st) || errno!=ENOENT || rename(zip,zipstash))) {rename(stash,source);return 125;}
 if(!lstat(source,&st) || errno!=ENOENT || (z && (!lstat(zip,&st) || errno!=ENOENT))) {if(z)rename(zipstash,zip);rename(stash,source);return 125;}
 pid_t child=fork();int status=0,rc=125;
 if(child==0) {execv(image,&argv[6]);_exit(126);}
 if(child>0 && waitpid(child,&status,0)==child) rc=WIFEXITED(status)?WEXITSTATUS(status):128+WTERMSIG(status);
 if(z && rename(zipstash,zip))rc=125;
 if(rename(stash,source))rc=125;
 return rc;
}
