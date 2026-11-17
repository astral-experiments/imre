#define _GNU_SOURCE
#include <fcntl.h>
#include <link.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
unsigned int la_version(unsigned int v) { return LAV_CURRENT; }
unsigned int la_objopen(struct link_map *map,Lmid_t ns,uintptr_t *cookie) {
    const char *path=getenv("AUDITLIBS_LOG"), *name=map->l_name;
    int fd=path?open(path,O_WRONLY|O_CREAT|O_APPEND|O_CLOEXEC,0600):-1;
    if (fd<0) _exit(124);
    if (!name || !*name) name="<main executable>";
    write(fd,name,strlen(name)); write(fd,"\n",1); close(fd);
    if (strstr(name,"libpython")) _exit(126);
    return 0;
}
