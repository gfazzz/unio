#include <stdio.h>
#include <fcntl.h>
#include <errno.h>
#include <string.h>
#include <unistd.h>
#include <sys/stat.h>
#include <dirent.h>
#include <stddef.h>
static void o(const char *what, const char *p, int fl) {
    errno = 0; int fd = open(p, fl);
    printf("open %-40s %-30s flags=0x%08x -> %d errno=%d %s\n", what, p, fl, fd, fd < 0 ? errno : 0, fd < 0 ? strerror(errno) : "");
    if (fd >= 0) close(fd);
}
static void s(const char *what, const char *p, int fl) {
    struct stat st; errno = 0; int r = fstatat(AT_FDCWD, p, &st, fl);
    printf("fstatat %-37s %-30s flags=0x%04x -> %d errno=%d %s mode=%o\n", what, p, fl, r, r < 0 ? errno : 0, r < 0 ? strerror(errno) : "", r==0? st.st_mode:0);
}
int main(int argc, char **argv) {
    printf("O_NOFOLLOW=0x%x O_NOFOLLOW_ANY=0x%x O_DIRECTORY=0x%x O_CLOEXEC=0x%x AT_FDCWD=%d AT_SYMLINK_NOFOLLOW=0x%x AT_SYMLINK_NOFOLLOW_ANY=0x%x\n",
        O_NOFOLLOW, O_NOFOLLOW_ANY, O_DIRECTORY, O_CLOEXEC, AT_FDCWD, AT_SYMLINK_NOFOLLOW, AT_SYMLINK_NOFOLLOW_ANY);
#ifdef O_RESOLVE_BENEATH
    printf("O_RESOLVE_BENEATH=0x%x\n", O_RESOLVE_BENEATH);
#endif
#ifdef AT_RESOLVE_BENEATH
    printf("AT_RESOLVE_BENEATH=0x%x\n", AT_RESOLVE_BENEATH);
#endif
    printf("sizeof stat=%zu off st_mode=%zu sizeof dirent=%zu off d_name=%zu\n", sizeof(struct stat), offsetof(struct stat, st_mode), sizeof(struct dirent), offsetof(struct dirent, d_name));
    char cwd[1024]; getcwd(cwd, sizeof cwd);
    char f[1100]; snprintf(f, sizeof f, "%s/README.md", cwd);
    const char *paths[] = { f, cwd, "README.md", "." };
    for (int i = 0; i < 4; i++) {
        const char *p = paths[i];
        o("RDONLY", p, O_RDONLY);
        o("RDONLY|CLOEXEC|NOFOLLOW", p, O_RDONLY|O_CLOEXEC|O_NOFOLLOW);
        o("RDONLY|CLOEXEC|NOFOLLOW_ANY", p, O_RDONLY|O_CLOEXEC|O_NOFOLLOW_ANY);
        o("RDONLY|CLOEXEC|NOFOLLOW|NOFOLLOW_ANY", p, O_RDONLY|O_CLOEXEC|O_NOFOLLOW|O_NOFOLLOW_ANY);
        o("RDONLY|DIRECTORY|CLOEXEC|NOFOLLOW_ANY", p, O_RDONLY|O_DIRECTORY|O_CLOEXEC|O_NOFOLLOW_ANY);
        o("RDONLY|DIRECTORY|CLOEXEC|NOFOLLOW|ANY", p, O_RDONLY|O_DIRECTORY|O_CLOEXEC|O_NOFOLLOW|O_NOFOLLOW_ANY);
        s("0", p, 0);
        s("NOFOLLOW", p, AT_SYMLINK_NOFOLLOW);
        s("NOFOLLOW_ANY", p, AT_SYMLINK_NOFOLLOW_ANY);
        s("NOFOLLOW|NOFOLLOW_ANY", p, AT_SYMLINK_NOFOLLOW|AT_SYMLINK_NOFOLLOW_ANY);
    }
    /* link tests */
    mkdir("ptmp", 0700); mkdir("ptmp/real", 0700);
    FILE *w = fopen("ptmp/real/x", "w"); fputs("x", w); fclose(w);
    symlink("real", "ptmp/link"); symlink("x", "ptmp/real/lx");
    o("link mid NOFOLLOW_ANY", "ptmp/link/x", O_RDONLY|O_CLOEXEC|O_NOFOLLOW_ANY);
    o("link last NOFOLLOW_ANY", "ptmp/real/lx", O_RDONLY|O_CLOEXEC|O_NOFOLLOW_ANY);
    o("link last dir NOFOLLOW_ANY", "ptmp/link", O_RDONLY|O_DIRECTORY|O_CLOEXEC|O_NOFOLLOW_ANY);
    s("link mid NOFOLLOW_ANY", "ptmp/link/x", AT_SYMLINK_NOFOLLOW_ANY);
    s("link last NOFOLLOW_ANY", "ptmp/real/lx", AT_SYMLINK_NOFOLLOW_ANY);
    s("link last NOFOLLOW", "ptmp/real/lx", AT_SYMLINK_NOFOLLOW);
    s("link mid NOFOLLOW", "ptmp/link/x", AT_SYMLINK_NOFOLLOW);
    return 0;
}
