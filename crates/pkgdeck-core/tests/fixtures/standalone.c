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
/* Copy <argv0>.prefix (how a tool names itself in --version) into out. */
static void copy_prefix(const char *argv0, FILE *out) {
    char path[4096];
    snprintf(path, sizeof(path), "%s.prefix", argv0);
    FILE *f = fopen(path, "r");
    if (!f) return;
    int c; while ((c = fgetc(f)) != EOF) fputc(c, out);
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
    if (argc == 3 && strcmp(argv[1], "check-update") == 0) {
        snprintf(path, sizeof(path), "%s.check.json", argv[0]);
        print_file(path, "{\"update_available\":true,\"latest_version\":\"2.0.0\",\"error\":null}");
        return 0;
    }
    /* Keep each run in <argv0>.runs. A <argv0>.fail naming argv[1] fails it. */
    if (argc >= 2) {
        snprintf(path, sizeof(path), "%s.runs", argv[0]);
        FILE *runs = fopen(path, "a"); if (!runs) return 1;
        for (int i=1; i<argc; ++i) fprintf(runs, "%s ", argv[i]);
        const char *track = getenv("DO_NOT_TRACK");
        fprintf(runs, "DNT=%s\n", track ? track : "");
        fclose(runs);
        snprintf(path, sizeof(path), "%s.fail", argv[0]);
        FILE *fail = fopen(path, "r");
        if (fail) {
            char word[64] = "";
            int read = fscanf(fail, "%63s", word);
            fclose(fail);
            if (read == 1 && strcmp(word, argv[1]) == 0) return 4;
        }
    }
    snprintf(path, sizeof(path), "%s.args", argv[0]);
    FILE *f = fopen(path, "w"); if (!f) return 1;
    for (int i=1; i<argc; ++i) fprintf(f, "%s\n", argv[i]);
    fclose(f);
    snprintf(path, sizeof(path), "%s.version", argv[0]);
    f = fopen(path, "w"); if (!f) return 1;
    copy_prefix(argv[0], f);
    fputs("2.0.0\n", f); fclose(f);
    puts("Synthetic update complete");
    return 0;
}
