/* Fork on either side of a foreign sigaction without waiting for that pthread. */
#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <stdlib.h>
#include <sys/wait.h>
#include <unistd.h>

int imre_fault_register(int, int, int, int);
int imre_fault_unregister(int);
int __real_sigaction(int, const struct sigaction *, struct sigaction *);

static int ready[2], resume[2], fd;
static _Thread_local int pause_signal;
static struct sigaction installed;

int __wrap_sigaction(int sig, const struct sigaction *action, struct sigaction *old) {
    int result = __real_sigaction(sig, action, old);
    if (action && sig == pause_signal) {
        char byte;
        if (sig == SIGUSR2) installed = *action;
        pause_signal = 0;
        if (write(ready[1], "1", 1) != 1 || read(resume[0], &byte, 1) != 1) abort();
    }
    return result;
}

static void *worker(void *argument) {
    int sig = *(int *)argument;
    pause_signal = sig;
    if (sig == SIGUSR1) {
        if (raise(sig)) abort();  /* Pinned while restoring the predecessor. */
    } else if (imre_fault_register(sig, fd, 0, 0)) {
        abort();  /* The new kernel action is installed but registration is incomplete. */
    }
    return NULL;
}

int main(void) {
    fd = open("/dev/null", O_WRONLY);
    if (fd < 0 || pipe(ready) || pipe(resume)) return 1;
    if (signal(SIGUSR1, SIG_IGN) == SIG_ERR || signal(SIGUSR2, SIG_IGN) == SIG_ERR) return 1;
    if (imre_fault_register(SIGUSR1, fd, 0, 1)) return 1;
    int cases[] = { SIGUSR1, SIGUSR2 };
    for (size_t i = 0; i < sizeof(cases)/sizeof(*cases); i++) {
        int sig = cases[i], status;
        pthread_t thread;
        char byte;
        if (pthread_create(&thread, NULL, worker, &sig) || read(ready[0], &byte, 1) != 1) return 1;
        pid_t child = fork();
        if (child < 0) return 1;
        if (!child) {
            alarm(5);
            if (imre_fault_unregister(SIGUSR1) != 1 || imre_fault_unregister(SIGUSR2) != 0) _exit(1);
            struct sigaction action;
            if (sigaction(sig, NULL, &action) || raise(sig)) _exit(1);
            if (action.sa_handler != (sig == SIGUSR1 ? SIG_IGN : installed.sa_handler)) _exit(1);
            if (sig == SIGUSR2) {
                if (imre_fault_register(sig, fd, 0, 1) || raise(sig) ||
                    imre_fault_unregister(sig) != 1 || raise(sig)) _exit(1);
            }
            _exit(37);
        }
        if (waitpid(child, &status, 0) != child || write(resume[1], "1", 1) != 1) return 1;
        if (pthread_join(thread, NULL) || !WIFEXITED(status) || WEXITSTATUS(status) != 37) return 1;
    }
    return imre_fault_unregister(SIGUSR1) != 1 || imre_fault_unregister(SIGUSR2) != 1;
}
