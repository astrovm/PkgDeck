/* Synthetic standalone CLI: never accesses network or non-fixture paths. */
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
static void print_file(const char *path, const char *fallback) {
    FILE *f = fopen(path, "r");
    if (!f) { puts(fallback); return; }
    int c; while ((c = fgetc(f)) != EOF) putchar(c);
    fclose(f);
}
int main(int argc, char **argv) {
    char path[4096];
    if (argc == 2 && strcmp(argv[1], "--version") == 0) {
        snprintf(path, sizeof(path), "%s.version", argv[0]);
        print_file(path, "1.0.0");
        return 0;
    }
    if (argc == 4 && strcmp(argv[2], "--check") == 0) {
        snprintf(path, sizeof(path), "%s.check.json", argv[0]);
        print_file(path, "{\"installer\":\"internal\",\"updateAvailable\":true,\"latestVersion\":\"2.0.0\",\"error\":null}");
        return 0;
    }
    snprintf(path, sizeof(path), "%s.args", argv[0]);
    FILE *f = fopen(path, "w"); if (!f) return 1;
    for (int i=1; i<argc; ++i) fprintf(f, "%s\n", argv[i]);
    fclose(f);
    snprintf(path, sizeof(path), "%s.version", argv[0]);
    f = fopen(path, "w"); if (!f) return 1;
    fputs("2.0.0\n", f); fclose(f);
    puts("Synthetic update complete");
    return 0;
}
