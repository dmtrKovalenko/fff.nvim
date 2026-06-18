/*
 * Smoke test for libfff_c — the smallest possible end-to-end exercise of
 * the public C API. We:
 *
 *   1. Create a picker with an `FffCreateOptions` populated via C99
 *      designated initializers (the recommended idiom for direct C use).
 *   2. Wait for the initial scan to complete.
 *   3. Search for "smoke.c".
 *   4. Fail unless this very file appears in the results.
 *
 * Build + run via `make test-c-smoke`. Override $(CC) to test other
 * compilers.
 */

#include <fff.h>
#include <stdio.h>
#include <string.h>

int main(int argc, char **argv) {
    const char *base_path = argc > 1 ? argv[1] : ".";

    // make sure that FFF C api is designed more for FFI rather than for direct C usage (I'm sorry)
    struct FffResult *create_result = fff_create_instance_with(&(struct FffCreateOptions){
        .version = FFF_CREATE_OPTIONS_VERSION,
        .base_path = base_path,
        .enable_mmap_cache = false,
        .enable_content_indexing = false,
        .watch = false,
    });

    if (!create_result->success) {
        fprintf(stderr, "fff couldn't create instance: %s\n",
                create_result->error ? create_result->error : "?");
        fff_free_result(create_result);
        return 1;
    }

    void *file_picker = create_result->handle;
    fff_free_result(create_result); // safe to drop now: handle outlives the envelope

    struct FffResult *scan_result = fff_wait_for_scan(file_picker, 5000);
    if (!scan_result->success) {
        fprintf(stderr, "wait_for_scan failed: %s\n",
                scan_result->error ? scan_result->error : "?");
        fff_free_result(scan_result);
        fff_destroy(file_picker);
        return 1;
    }
    // int_value: 1 = scan completed in time, 0 = timed out.
    if (scan_result->int_value == 0) {
        fprintf(stderr, "wait_for_scan: timed out before initial scan finished\n");
        fff_free_result(scan_result);
        fff_destroy(file_picker);
        return 1;
    }
    fff_free_result(scan_result);

    struct FffResult *res = fff_search(file_picker, "smkoe.c", "", 0, 0, 50, 0, 0);
    if (!res->success) {
        fprintf(stderr, "search failed: %s\n", res->error ? res->error : "?");
        fff_free_result(res);
        fff_destroy(file_picker);
        return 1;
    }

    struct FffSearchResult *sr = (struct FffSearchResult *)res->handle;
    uint32_t total = sr->count;
    int found = 0;
    for (uint32_t i = 0; i < sr->count; i++) {
        const char *path = sr->items[i].relative_path;
        if (path && strstr(path, "smoke.c")) {
            found = 1;
            fprintf(stderr, "found self: %s\n", path);
            break;
        }
    }

    fff_free_search_result(sr);
    fff_free_result(res);
    fff_destroy(file_picker);

    if (!found) {
        fprintf(stderr, "FAIL: smoke.c not in search results (count=%u)\n", total);
        return 1;
    }

    fprintf(stderr, "PASS\n");
    return 0;
}
