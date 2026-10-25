/* Linux fixture: make the kernel report EBADF by closing the selected directory
 * descriptor twice. Only the two directories in this private fixture qualify. */
#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

/* scandir owns a duplicate file description. The walker's os.close happens
 * later, after the iterator and its duplicate have gone out of scope. */
static int last_directory_fd(int fd, const struct stat *target) {
    DIR *fds = opendir("/proc/self/fd");
    if (!fds) _exit(72);
    int last = 1;
    struct dirent *entry;
    while ((entry = readdir(fds))) {
        if (entry->d_name[0] < '0' || entry->d_name[0] > '9') continue;
        int other = atoi(entry->d_name);
        struct stat st;
        if (other != fd && fstat(other, &st) == 0 && st.st_dev == target->st_dev && st.st_ino == target->st_ino) {
            last = 0;
            break;
        }
    }
    closedir(fds);
    return last;
}

int close(int fd) {
    const char *root = getenv("IMRE_SHUTIL_CLOSE_ROOT");
    const char *where = getenv("IMRE_SHUTIL_CLOSE_FAULT");
    const char *log = getenv("IMRE_SHUTIL_CLOSE_LOG_FD");
    struct stat st;
    char proc[64], path[PATH_MAX];
    const char *entry = NULL;
    if (root && where && log && fstat(fd, &st) == 0 && S_ISDIR(st.st_mode)) {
        snprintf(proc, sizeof(proc), "/proc/self/fd/%d", fd);
        ssize_t n = readlink(proc, path, sizeof(path) - 1);
        if (n > 0) {
            path[n] = '\0';
            size_t prefix = strlen(root);
            if (strcmp(path, root) == 0) entry = "root";
            else if (strncmp(path, root, prefix) == 0 && strcmp(path + prefix, "/child") == 0)
                entry = "root/child";
        }
    }
    if (entry && !last_directory_fd(fd, &st)) entry = NULL;
    int status = (int)syscall(SYS_close, fd);
    if (!entry) return status;
    if (status == 0 && (strcmp(where, "both") == 0 || strcmp(entry, "root/child") == 0))
        status = (int)syscall(SYS_close, fd);
    int saved = errno;
    if (dprintf(atoi(log), "%s\n", entry) < 0) _exit(73);
    errno = saved;
    return status;
}
