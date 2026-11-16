/* Fork while another thread is growing its frame buffer under the slot gate. */
#include <pthread.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/wait.h>
#include <unistd.h>

void imre_fault_frame(size_t, const char *, size_t, const char *, size_t, uint32_t);
void imre_fault_dump(int, int);
void *__real_realloc(void *, size_t);

static int ready[2], resume[2];
static _Thread_local int growing;

void *__wrap_realloc(void *ptr, size_t size) {
    if (growing) {
        char byte;
        if (write(ready[1], "1", 1) != 1 || read(resume[0], &byte, 1) != 1) abort();
        growing = 0;
    }
    return __real_realloc(ptr, size);
}

static void *publisher(void *unused) {
    (void)unused;
    imre_fault_frame(0, "parent.py", 9, "outer", 5, 10);
    growing = 1;
    imre_fault_frame(1, "parent.py", 9, "inner", 5, 20);
    imre_fault_dump(STDOUT_FILENO, 0);
    return NULL;
}

int main(void) {
    pthread_t thread;
    char byte;
    if (pipe(ready) || pipe(resume) || pthread_create(&thread, NULL, publisher, NULL)) return 1;
    if (read(ready[0], &byte, 1) != 1) return 1;
    pid_t child = fork();
    if (child < 0) return 1;
    if (child == 0) {
        alarm(5);
        imre_fault_frame(0, "child.py", 8, "child", 5, 30);
        imre_fault_dump(STDOUT_FILENO, 0);
        _exit(0);
    }
    int status;
    if (waitpid(child, &status, 0) != child || write(resume[1], "1", 1) != 1) return 1;
    if (pthread_join(thread, NULL)) return 1;
    return !WIFEXITED(status) || WEXITSTATUS(status) != 0;
}
