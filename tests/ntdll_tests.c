/*
 * Regression tests for the ntdll patches that change what a title can
 * observe: the mapped-image filename fallback (patches/wine/0010), the code
 * bytes dumped for a fault under +seh (0007) and FileAllocationInformation
 * (0012).
 *
 * One mode per run, because some need a particular environment:
 *   ntdll_tests mapped-name        - an ordinary process. The main image has
 *                                    a real file name, so a short buffer gets
 *                                    STATUS_BUFFER_OVERFLOW, as on Windows.
 *   ntdll_tests mapped-name-memfd  - the main image mapped from a memfd
 *                                    through WINE_DLL_FILE_MAP: the name comes
 *                                    from the PEB, and a PEB path the title
 *                                    has broken must not crash ntdll.
 *   ntdll_tests seh-dump           - run with WINEDEBUG=+seh: faults whose
 *                                    surrounding bytes are unreadable must
 *                                    reach the title's vectored handler once,
 *                                    not again from inside ntdll's dump.
 *   ntdll_tests alloc              - FileAllocationInformation beyond end of
 *                                    file does not leave space reserved after
 *                                    the handle is closed; below end of file it
 *                                    truncates.
 */

#include <windows.h>
#include <winternl.h>
#include <stdio.h>
#include <string.h>
#include <wchar.h>

static int fails;

#define OK(cond, ...) do { \
    if (cond) { printf("ok   - "); printf(__VA_ARGS__); printf("\n"); } \
    else { printf("FAIL - "); printf(__VA_ARGS__); printf("\n"); fails++; } \
} while (0)

#ifndef STATUS_BUFFER_OVERFLOW
#define STATUS_BUFFER_OVERFLOW ((NTSTATUS)0x80000005)
#endif
#ifndef STATUS_INFO_LENGTH_MISMATCH
#define STATUS_INFO_LENGTH_MISMATCH ((NTSTATUS)0xC0000004)
#endif

/* MemoryMappedFilenameInformation, declared here because the mingw headers
 * do not all carry it. */
#define MAPPED_FILENAME_CLASS 2
struct section_name
{
    UNICODE_STRING name;
    WCHAR buffer[1024];
};

typedef NTSTATUS (WINAPI *query_vm_fn)(HANDLE, const void *, int, void *, SIZE_T, SIZE_T *);
typedef NTSTATUS (WINAPI *set_file_fn)(HANDLE, IO_STATUS_BLOCK *, void *, ULONG, int);

static query_vm_fn query_vm;
static set_file_fn set_file;

#define FILE_ALLOCATION_CLASS 19
#ifndef STATUS_ACCESS_VIOLATION
#define STATUS_ACCESS_VIOLATION ((NTSTATUS)0xC0000005)
#endif

static NTSTATUS query_name(void *addr, struct section_name *out, SIZE_T len, SIZE_T *ret)
{
    *ret = 0;
    memset(out, 0, sizeof(*out));
    return query_vm(GetCurrentProcess(), addr, MAPPED_FILENAME_CLASS, out, len, ret);
}

static BOOL name_ends_with_exe(const UNICODE_STRING *name)
{
    WCHAR exe[MAX_PATH], *base;
    size_t n, have = name->Length / sizeof(WCHAR);

    GetModuleFileNameW(NULL, exe, MAX_PATH);
    base = wcsrchr(exe, '\\') ? wcsrchr(exe, '\\') + 1 : exe;
    n = wcslen(base);
    return have >= n && !_wcsnicmp(name->Buffer + have - n, base, n);
}

static void test_mapped_name(void)
{
    struct section_name info;
    void *image = GetModuleHandleW(NULL);
    SIZE_T ret;
    NTSTATUS st;

    st = query_name(image, &info, sizeof(info), &ret);
    OK(st == 0, "main image name -> SUCCESS (got %#lx)", st);
    OK(st == 0 && name_ends_with_exe(&info.name), "main image name is the exe (%.*ls)",
       (int)(info.name.Length / sizeof(WCHAR)), info.name.Buffer ? info.name.Buffer : L"");

    /* Room for the header and a few characters, not for the name. The file
     * has a name, so this is the server's answer, STATUS_BUFFER_OVERFLOW,
     * and not one made up from the PEB. */
    st = query_name(image, &info, sizeof(UNICODE_STRING) + 8, &ret);
    OK(st == STATUS_BUFFER_OVERFLOW, "short buffer -> STATUS_BUFFER_OVERFLOW (got %#lx)", st);
    OK(ret > sizeof(UNICODE_STRING) + 8, "short buffer reports the size it needs (%lu)", (unsigned long)ret);
}

static void test_mapped_name_memfd(void)
{
    RTL_USER_PROCESS_PARAMETERS *params = NtCurrentTeb()->ProcessEnvironmentBlock->ProcessParameters;
    struct section_name info;
    void *image = GetModuleHandleW(NULL);
    PWSTR saved;
    SIZE_T ret;
    NTSTATUS st;

    st = query_name(image, &info, sizeof(info), &ret);
    OK(st == 0, "memfd image name -> SUCCESS from the PEB (got %#lx)", st);
    OK(st == 0 && name_ends_with_exe(&info.name), "memfd image name is the exe (%.*ls)",
       (int)(info.name.Length / sizeof(WCHAR)), info.name.Buffer ? info.name.Buffer : L"");

    st = query_name(image, &info, sizeof(UNICODE_STRING) + 8, &ret);
    OK(st == STATUS_INFO_LENGTH_MISMATCH, "memfd short buffer -> INFO_LENGTH_MISMATCH (got %#lx)", st);
    OK(ret > sizeof(UNICODE_STRING) + params->ImagePathName.Length,
       "memfd short buffer reports the size it needs (%lu)", (unsigned long)ret);

    /* The process parameters are the title's to write. A path pointer it has
     * broken has to mean "no answer", not a fault in the Unix side of ntdll.
     * Nothing else in this test runs until it is put back. */
    saved = params->ImagePathName.Buffer;
    params->ImagePathName.Buffer = (PWSTR)(ULONG_PTR)0x1000;
    st = query_name(image, &info, sizeof(info), &ret);
    params->ImagePathName.Buffer = saved;
    OK(st != 0 && st != STATUS_ACCESS_VIOLATION,
       "a broken PEB image path means no name, not a fault in the syscall (got %#lx)", st);
}

static LONG real_av, ill, stray_av;
static void *ud2_addr;

static LONG CALLBACK count_faults(EXCEPTION_POINTERS *ep)
{
    EXCEPTION_RECORD *rec = ep->ExceptionRecord;
    CONTEXT *ctx = ep->ContextRecord;

    if (rec->ExceptionCode == EXCEPTION_ACCESS_VIOLATION && rec->ExceptionAddress == (void *)0x18)
    {
        /* A call through a null vtable: return to the caller. */
        real_av++;
        ctx->Rip = *(DWORD64 *)ctx->Rsp;
        ctx->Rsp += 8;
        return EXCEPTION_CONTINUE_EXECUTION;
    }
    if (rec->ExceptionCode == EXCEPTION_ILLEGAL_INSTRUCTION && rec->ExceptionAddress == ud2_addr)
    {
        ill++;
        ctx->Rip += 2;
        return EXCEPTION_CONTINUE_EXECUTION;
    }
    if (rec->ExceptionCode == EXCEPTION_ACCESS_VIOLATION)
    {
        printf("     stray access violation at %p (reading %p)\n", rec->ExceptionAddress,
               (void *)rec->ExceptionInformation[1]);
        stray_av++;
    }
    return EXCEPTION_CONTINUE_SEARCH;
}

static void test_seh_dump(void)
{
    void (*volatile null_vtable_call)(void) = (void (*)(void))(ULONG_PTR)0x18;
    void (*code_fn)(void);
    BYTE *region, *code;
    void *veh;

    veh = AddVectoredExceptionHandler(1, count_faults);

    /* Execute fault at 0x18: the 32 bytes "before" it wrap around the top
     * of the address space. */
    null_vtable_call();
    OK(real_av == 1, "null-vtable call seen once by the handler (%ld)", real_av);
    OK(stray_av == 0, "no second fault from dumping its code bytes (%ld)", stray_av);

    /* ud2 as the first bytes of a committed page after a reserved one: the
     * bytes before it cannot be read, the ones from it can. */
    region = VirtualAlloc(NULL, 0x20000, MEM_RESERVE, PAGE_NOACCESS);
    code = VirtualAlloc(region + 0x10000, 0x1000, MEM_COMMIT, PAGE_EXECUTE_READWRITE);
    OK(region && code, "allocated the ud2 page");
    if (code)
    {
        static const BYTE ud2_ret[] = { 0x0f, 0x0b, 0xc3 };

        memcpy(code, ud2_ret, sizeof(ud2_ret));
        FlushInstructionCache(GetCurrentProcess(), code, sizeof(ud2_ret));
        ud2_addr = code;
        stray_av = 0;
        code_fn = (void (*)(void))code;
        code_fn();
        OK(ill == 1, "ud2 seen once by the handler (%ld)", ill);
        OK(stray_av == 0, "no fault from reading the reserved page before it (%ld)", stray_av);
    }
    RemoveVectoredExceptionHandler(veh);
    if (region) VirtualFree(region, 0, MEM_RELEASE);
}

static void test_alloc(void)
{
    static const WCHAR path[] = L"ntdll-alloc-test.bin";
    FILE_ALLOCATION_INFO alloc;
    FILE_STANDARD_INFO std;
    IO_STATUS_BLOCK io;
    char data[4096];
    DWORD written;
    NTSTATUS st;
    HANDLE h;
    BOOL r;

    h = CreateFileW(path, GENERIC_READ | GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    OK(h != INVALID_HANDLE_VALUE, "created the test file (%lu)", GetLastError());
    if (h == INVALID_HANDLE_VALUE) return;

    /* NtSetInformationFile directly: kernelbase does not pass this class
     * through SetFileInformationByHandle. */
    alloc.AllocationSize.QuadPart = 64 << 20;
    st = set_file(h, &io, &alloc, sizeof(alloc), FILE_ALLOCATION_CLASS);
    OK(st == 0, "growing the allocation succeeds (%#lx)", st);
    CloseHandle(h);

    /* NTFS gives unused allocation past end of file back when the last
     * handle closes. Here nothing may still be held once it has. */
    h = CreateFileW(path, GENERIC_READ | GENERIC_WRITE, 0, NULL, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, NULL);
    r = GetFileInformationByHandleEx(h, FileStandardInfo, &std, sizeof(std));
    OK(r && std.EndOfFile.QuadPart == 0, "end of file is still 0 (%lld)", std.EndOfFile.QuadPart);
    OK(r && std.AllocationSize.QuadPart < (64 << 20),
       "no 64 MiB reservation left after close (allocation %lld)", std.AllocationSize.QuadPart);

    /* Below end of file it truncates, as on Windows. */
    memset(data, 'x', sizeof(data));
    WriteFile(h, data, sizeof(data), &written, NULL);
    alloc.AllocationSize.QuadPart = 100;
    st = set_file(h, &io, &alloc, sizeof(alloc), FILE_ALLOCATION_CLASS);
    OK(st == 0, "shrinking the allocation succeeds (%#lx)", st);
    r = GetFileInformationByHandleEx(h, FileStandardInfo, &std, sizeof(std));
    OK(r && std.EndOfFile.QuadPart == 100, "shrinking truncates to 100 (%lld)", std.EndOfFile.QuadPart);
    CloseHandle(h);
    DeleteFileW(path);
}

int main(int argc, char **argv)
{
    const char *mode = argc > 1 ? argv[1] : "";

    query_vm = (query_vm_fn)GetProcAddress(GetModuleHandleW(L"ntdll.dll"), "NtQueryVirtualMemory");
    set_file = (set_file_fn)GetProcAddress(GetModuleHandleW(L"ntdll.dll"), "NtSetInformationFile");

    if (!strcmp(mode, "mapped-name")) test_mapped_name();
    else if (!strcmp(mode, "mapped-name-memfd")) test_mapped_name_memfd();
    else if (!strcmp(mode, "seh-dump")) test_seh_dump();
    else if (!strcmp(mode, "alloc")) test_alloc();
    else { printf("usage: ntdll_tests mapped-name|mapped-name-memfd|seh-dump|alloc\n"); return 2; }

    if (fails) { printf("\nFAILED (%d)\n", fails); return 1; }
    printf("\nPASSED\n");
    return 0;
}
