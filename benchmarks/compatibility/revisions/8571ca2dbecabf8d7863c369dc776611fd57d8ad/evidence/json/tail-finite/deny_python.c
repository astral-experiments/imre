#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <spawn.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
static int forbidden(const char *path) {
 if (!path) return 0;
 const char *name=strrchr(path,'/'); name=name?name+1:path;
 return !strncmp(name,"python",6) || strstr(name,"libpython") != NULL;
}
static int deny(const char *path) { if (!forbidden(path)) return 0; dprintf(2,"JSON_CONTROL_DENIED %s\n",path); errno=EACCES; return 1; }
int execve(const char *p,char *const a[],char *const e[]) { if(deny(p))return -1; return ((int(*)(const char*,char *const[],char *const[]))dlsym(RTLD_NEXT,"execve"))(p,a,e); }
int execveat(int fd,const char *p,char *const a[],char *const e[],int f) { if(deny(p))return -1; return ((int(*)(int,const char*,char *const[],char *const[],int))dlsym(RTLD_NEXT,"execveat"))(fd,p,a,e,f); }
int execv(const char *p,char *const a[]) { if(deny(p))return -1; return ((int(*)(const char*,char *const[]))dlsym(RTLD_NEXT,"execv"))(p,a); }
int execvp(const char *p,char *const a[]) { if(deny(p))return -1; return ((int(*)(const char*,char *const[]))dlsym(RTLD_NEXT,"execvp"))(p,a); }
int execvpe(const char *p,char *const a[],char *const e[]) { if(deny(p))return -1; return ((int(*)(const char*,char *const[],char *const[]))dlsym(RTLD_NEXT,"execvpe"))(p,a,e); }
int posix_spawn(pid_t *pid,const char *p,const posix_spawn_file_actions_t *fa,const posix_spawnattr_t *at,char *const a[],char *const e[]) { if(deny(p))return EACCES; return ((int(*)(pid_t*,const char*,const posix_spawn_file_actions_t*,const posix_spawnattr_t*,char *const[],char *const[]))dlsym(RTLD_NEXT,"posix_spawn"))(pid,p,fa,at,a,e); }
int posix_spawnp(pid_t *pid,const char *p,const posix_spawn_file_actions_t *fa,const posix_spawnattr_t *at,char *const a[],char *const e[]) { if(deny(p))return EACCES; return ((int(*)(pid_t*,const char*,const posix_spawn_file_actions_t*,const posix_spawnattr_t*,char *const[],char *const[]))dlsym(RTLD_NEXT,"posix_spawnp"))(pid,p,fa,at,a,e); }
void *dlopen(const char *p,int f) { if(deny(p))return NULL; return ((void*(*)(const char*,int))dlsym(RTLD_NEXT,"dlopen"))(p,f); }
void *dlmopen(Lmid_t lm,const char *p,int f) { if(deny(p))return NULL; return ((void*(*)(Lmid_t,const char*,int))dlsym(RTLD_NEXT,"dlmopen"))(lm,p,f); }
