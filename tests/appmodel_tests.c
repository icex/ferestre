/*
 * Regression tests for the current-process package-identity implementation in
 * kernelbase/package.c (the fix for UWP apps aborting with
 * APPMODEL_ERROR_NO_PACKAGE).
 *
 * Run in two modes:
 *   appmodel_tests packaged   - WINE_PACKAGE_MANIFEST points at the fixture;
 *                               expect a fully derived identity.
 *   appmodel_tests bare       - no manifest anywhere; expect NO_PACKAGE.
 *
 * Expected strings come from the environment (EXPECT_FAMILY/EXPECT_FULL/
 * EXPECT_AUMID) so the test runner stays the single source of truth.
 */

#include <windows.h>
#include <appmodel.h>
#include <stdio.h>
#include <string.h>

static int fails;

#define OK(cond, ...) do { \
    if (cond) { printf("ok   - "); printf(__VA_ARGS__); printf("\n"); } \
    else { printf("FAIL - "); printf(__VA_ARGS__); printf("\n"); fails++; } \
} while (0)

static void getenv_w(const char *name, WCHAR *out, int cap)
{
    WCHAR wname[64];
    MultiByteToWideChar(CP_UTF8, 0, name, -1, wname, 64);
    if (!GetEnvironmentVariableW(wname, out, cap)) out[0] = 0;
}

static void test_packaged(void)
{
    WCHAR buf[512], expect_family[512], expect_full[512], expect_aumid[512];
    BYTE idbuf[1024];
    UINT32 len;
    LONG r;

    getenv_w("EXPECT_FAMILY", expect_family, 512);
    getenv_w("EXPECT_FULL",   expect_full,   512);
    getenv_w("EXPECT_AUMID",  expect_aumid,  512);

    /* Length-query pattern: NULL buffer must report required size. */
    len = 0;
    r = GetCurrentPackageFamilyName(&len, NULL);
    OK(r == ERROR_INSUFFICIENT_BUFFER, "family length query -> INSUFFICIENT_BUFFER (got %ld)", r);
    OK(len == lstrlenW(expect_family) + 1, "family length query reports %u (want %u)",
       len, (UINT32)(lstrlenW(expect_family) + 1));

    len = ARRAYSIZE(buf);
    r = GetCurrentPackageFamilyName(&len, buf);
    OK(r == ERROR_SUCCESS, "GetCurrentPackageFamilyName -> SUCCESS (got %ld)", r);
    OK(!lstrcmpW(buf, expect_family), "family = %ls (want %ls)", buf, expect_family);

    len = ARRAYSIZE(buf);
    r = GetCurrentPackageFullName(&len, buf);
    OK(r == ERROR_SUCCESS, "GetCurrentPackageFullName -> SUCCESS (got %ld)", r);
    OK(!lstrcmpW(buf, expect_full), "full = %ls (want %ls)", buf, expect_full);

    len = ARRAYSIZE(buf);
    r = GetCurrentApplicationUserModelId(&len, buf);
    OK(r == ERROR_SUCCESS, "GetCurrentApplicationUserModelId -> SUCCESS (got %ld)", r);
    OK(!lstrcmpW(buf, expect_aumid), "aumid = %ls (want %ls)", buf, expect_aumid);

    len = ARRAYSIZE(buf);
    r = GetCurrentPackagePath(&len, buf);
    OK(r == ERROR_SUCCESS, "GetCurrentPackagePath -> SUCCESS (got %ld)", r);
    OK(buf[0] != 0, "package path is non-empty (%ls)", buf);

    /* Buffer too small must fail with the required length, not corrupt memory. */
    len = 2;
    r = GetCurrentPackageFullName(&len, buf);
    OK(r == ERROR_INSUFFICIENT_BUFFER, "full into tiny buffer -> INSUFFICIENT_BUFFER (got %ld)", r);
    OK(len == lstrlenW(expect_full) + 1, "tiny-buffer call still reports required %u", len);

    /* PACKAGE_ID round-trips the full name. */
    {
        UINT32 idlen = 0;
        PACKAGE_ID *id = (PACKAGE_ID *)idbuf;

        r = GetCurrentPackageId(&idlen, NULL);
        OK(r == ERROR_INSUFFICIENT_BUFFER, "id length query -> INSUFFICIENT_BUFFER (got %ld)", r);
        OK(idlen > 0 && idlen <= sizeof(idbuf), "id length query reports %u", idlen);

        idlen = sizeof(idbuf);
        r = GetCurrentPackageId(&idlen, idbuf);
        OK(r == ERROR_SUCCESS, "GetCurrentPackageId -> SUCCESS (got %ld)", r);
        OK(!lstrcmpW(id->publisherId, L"8wekyb3d8bbwe"),
           "id.publisherId = %ls (want 8wekyb3d8bbwe)", id->publisherId);
        OK(!lstrcmpW(id->name, L"Microsoft.MinecraftUWP"),
           "id.name = %ls (want Microsoft.MinecraftUWP)", id->name);
        OK(id->version.Major == 1 && id->version.Minor == 21,
           "id.version = %u.%u (want 1.21)", id->version.Major, id->version.Minor);
    }

    /* GetCurrentPackageInfo: size query, then a filled record. */
    {
        UINT32 sz = 0, cnt = 0;
        BYTE *pi;
        PACKAGE_INFO *info;

        r = GetCurrentPackageInfo(0, &sz, NULL, &cnt);
        OK(r == ERROR_INSUFFICIENT_BUFFER, "packageinfo size query -> INSUFFICIENT_BUFFER (got %ld)", r);
        OK(sz >= sizeof(PACKAGE_INFO) && cnt == 1, "packageinfo needs %u bytes, count %u", sz, cnt);

        pi = HeapAlloc(GetProcessHeap(), 0, sz);
        r = GetCurrentPackageInfo(0, &sz, pi, &cnt);
        OK(r == ERROR_SUCCESS, "GetCurrentPackageInfo -> SUCCESS (got %ld)", r);
        info = (PACKAGE_INFO *)pi;
        OK(!lstrcmpW(info->packageFullName, expect_full),
           "info.packageFullName = %ls", info->packageFullName);
        OK(!lstrcmpW(info->packageFamilyName, expect_family),
           "info.packageFamilyName = %ls", info->packageFamilyName);
        OK(!lstrcmpW(info->packageId.name, L"Microsoft.MinecraftUWP"),
           "info.packageId.name = %ls", info->packageId.name);
        OK(info->path && info->path[0], "info.path is non-empty (%ls)", info->path);
        HeapFree(GetProcessHeap(), 0, pi);
    }

    /* By-handle variants, current process. */
    len = ARRAYSIZE(buf);
    r = GetPackageFamilyName(GetCurrentProcess(), &len, buf);
    OK(r == ERROR_SUCCESS && !lstrcmpW(buf, expect_family),
       "GetPackageFamilyName(current) = %ls (got %ld)", buf, r);
}

static void test_bare(void)
{
    WCHAR buf[512];
    UINT32 len = ARRAYSIZE(buf);
    LONG r;

    r = GetCurrentPackageFullName(&len, buf);
    OK(r == APPMODEL_ERROR_NO_PACKAGE, "unpackaged: full name -> NO_PACKAGE (got %ld)", r);

    len = ARRAYSIZE(buf);
    r = GetCurrentPackageFamilyName(&len, buf);
    OK(r == APPMODEL_ERROR_NO_PACKAGE, "unpackaged: family -> NO_PACKAGE (got %ld)", r);

    len = ARRAYSIZE(buf);
    r = GetCurrentApplicationUserModelId(&len, buf);
    OK(r == APPMODEL_ERROR_NO_APPLICATION, "unpackaged: aumid -> NO_APPLICATION (got %ld)", r);
}

int main(int argc, char **argv)
{
    if (argc > 1 && !strcmp(argv[1], "bare")) test_bare();
    else test_packaged();

    if (fails) { printf("\nFAILED (%d)\n", fails); return 1; }
    printf("\nPASSED\n");
    return 0;
}
