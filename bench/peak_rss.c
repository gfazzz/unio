/* Runs a command and prints its peak resident memory in bytes and its wall time in nanoseconds,
 * from fork to the end of the child, to standard error as the last line "peak_rss BYTES NS".
 * Linux records the peak of the process that starts a program as part of the program's peak,
 * so measuring from Python would add Python's own memory; this launcher is a few hundred
 * kilobytes. Its time leaves out the start of a process by Python, which the start-up
 * benchmark would otherwise count. Used by bench/run.py; built with the system C compiler. */
#include <stdio.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static long long now_ns(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (long long)t.tv_sec * 1000000000LL + t.tv_nsec;
}

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: peak_rss COMMAND [ARG...]\n");
        return 2;
    }
    long long start = now_ns();
    pid_t pid = fork();
    if (pid < 0) {
        perror("fork");
        return 2;
    }
    if (pid == 0) {
        execvp(argv[1], argv + 1);
        perror(argv[1]);
        _exit(127);
    }
    int status = 0;
    struct rusage usage;
    if (wait4(pid, &status, 0, &usage) < 0) {
        perror("wait4");
        return 2;
    }
    long long elapsed = now_ns() - start;
#ifdef __APPLE__
    long long bytes = (long long)usage.ru_maxrss;
#else
    long long bytes = (long long)usage.ru_maxrss * 1024;
#endif
    fprintf(stderr, "peak_rss %lld %lld\n", bytes, elapsed);
    if (WIFEXITED(status)) return WEXITSTATUS(status);
    if (WIFSIGNALED(status)) return 128 + WTERMSIG(status);
    return 2;
}
