/* Account-free native PlayFab ABI fixture. No network or real saves. */
#define COBJMACROS
#include <windows.h>
#include <stdlib.h>
#include <string.h>
#include "xasync.h"

#define API __declspec(dllexport) HRESULT WINAPI
struct init_args { void *queue; UINT64 options; const char *folder; };
struct local_user { void *xuser; LONG final_uploads; };
struct ui_callbacks
{
    void (WINAPI *progress)(void *, UINT32, void *); void *context;
    void *remaining[8];
};
static struct ui_callbacks ui;
static LONG closed, final_uploads;
static const char folder[] = "C:\\pfx-fixture\\account-save";
static HRESULT result_code(void)
{
    char value[32];
    return GetEnvironmentVariableA("PFX_FIXTURE_RESULT", value, sizeof(value)) ? strtoul(value, NULL, 16) : S_OK;
}
API PFServiceConfigCreateHandle(const char *url, const char *title, void **out)
{ *out = malloc(1); return *out ? S_OK : E_OUTOFMEMORY; }
__declspec(dllexport) void WINAPI PFServiceConfigCloseHandle(void *h) { free(h); }
API PFLocalUserCreateHandleWithXboxUser(void *service, void *user, void *context, void **out)
{
    struct local_user *u = calloc(1, sizeof(*u));
    if (!u) return E_OUTOFMEMORY;
    u->xuser = user; *out = u; return S_OK;
}
__declspec(dllexport) void WINAPI PFLocalUserCloseHandle(void *h) { struct local_user *u = h; InterlockedExchangeAdd(&final_uploads, u->final_uploads); InterlockedIncrement(&closed); free(h); }
__declspec(dllexport) LONG WINAPI PFFixtureFinalUploads(void) { return final_uploads; }
__declspec(dllexport) LONG WINAPI PFFixtureClosedUsers(void) { return closed; }
__declspec(dllexport) INT32 WINAPI PFLocalUserHandleCompare(void *a, void *b)
{ return ((struct local_user *)a)->xuser == ((struct local_user *)b)->xuser ? 0 : 1; }
API PFGameSaveFilesInitialize(struct init_args *args)
{
    DWORD flag = 0, size = sizeof(flag);
    if (!args || !args->folder || !*args->folder || args->options ||
        RegGetValueA(HKEY_LOCAL_MACHINE, "Software\\Microsoft\\GamingServices",
                     "ForceUseInprocGameSaves", RRF_RT_REG_DWORD, NULL, &flag, &size) || flag != 1)
        return E_INVALIDARG;
    return S_OK;
}
struct work { XAsyncBlock *async; HRESULT hr; };
static DWORD WINAPI finish(void *context)
{
    struct work *w = context;
    XAsyncBlock *a = w->async;
    Sleep(40);
    a->internal[1] = (void *)(ULONG_PTR)w->hr;
    free(w);
    a->callback(a);
    return 0;
}
static HRESULT begin(XAsyncBlock *a)
{
    struct work *w = malloc(sizeof(*w));
    HANDLE thread;
    if (!w) return E_OUTOFMEMORY;
    w->async = a; w->hr = result_code();
    thread = CreateThread(NULL, 0, finish, w, 0, NULL);
    if (!thread) { free(w); return E_FAIL; }
    CloseHandle(thread); return S_OK;
}
API PFGameSaveFilesAddUserWithUiAsync(void *u, UINT32 flags, XAsyncBlock *a)
{
    if (ui.progress) ui.progress(u, 3, ui.context);
    return begin(a);
}
API PFGameSaveFilesAddUserWithUiResult(XAsyncBlock *a) { return (HRESULT)(ULONG_PTR)a->internal[1]; }
API PFGameSaveFilesGetFolderSize(void *u, SIZE_T *size) { *size = sizeof(folder); return S_OK; }
API PFGameSaveFilesGetFolder(void *u, SIZE_T size, char *out, SIZE_T *used)
{
    if (size < sizeof(folder)) return HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER);
    memcpy(out, folder, sizeof(folder)); if (used) *used = sizeof(folder); return S_OK;
}
API PFGameSaveFilesGetRemainingQuota(void *u, INT64 *out)
{ HRESULT hr = result_code(); if (SUCCEEDED(hr)) *out = 987654321; return hr; }
API PFGameSaveFilesIsConnectedToCloud(void *u, BOOLEAN *out)
{ char v[4]; *out = !GetEnvironmentVariableA("PFX_FIXTURE_OFFLINE", v, sizeof(v)); return S_OK; }
API PFGameSaveFilesUploadWithUiAsync(void *u, UINT32 options, XAsyncBlock *a) { if (options == 1) InterlockedIncrement(&((struct local_user *)u)->final_uploads); return begin(a); }
API PFGameSaveFilesUploadWithUiResult(XAsyncBlock *a) { return (HRESULT)(ULONG_PTR)a->internal[1]; }
API PFGameSaveFilesSetSaveDescriptionAsync(void *u, const char *s, XAsyncBlock *a) { return begin(a); }
API PFGameSaveFilesSetSaveDescriptionResult(XAsyncBlock *a) { return (HRESULT)(ULONG_PTR)a->internal[1]; }
API PFGameSaveFilesSetUiCallbacks(struct ui_callbacks *callbacks) { ui = *callbacks; return S_OK; }
API PFGameSaveFilesUiProgressGetProgress(void *u, UINT32 *s, UINT64 *c, UINT64 *t)
{ if (!s || !c || !t) return E_INVALIDARG; *s = 3; *c = 12; *t = 42; return S_OK; }
API PFGameSaveFilesSetUiProgressResponse(void *u, UINT32 response) { return result_code(); }
API PFGameSaveFilesSetUiSyncFailedResponse(void *u, UINT32 response) { return result_code(); }
API PFGameSaveFilesSetUiActiveDeviceContentionResponse(void *u, UINT32 response) { return result_code(); }
API PFGameSaveFilesSetUiConflictResponse(void *u, UINT32 response) { return result_code(); }
API PFGameSaveFilesSetUiOutOfStorageResponse(void *u, UINT32 response) { return result_code(); }
API PFGameSaveFilesSetActiveDeviceChangedCallback(void *queue, void *callback, void *context) { return S_OK; }
