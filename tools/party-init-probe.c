/* Initialize a title's shipped PlayFab Party DLL without launching its game.
 * Usage under Wine: party-init-probe.exe <Windows DLL directory> <PlayFab title id>
 * The directory and DLL stay in the licensed installation; no game is copied.
 */
#include <windows.h>
#include <stdio.h>

int main( int argc, char **argv )
{
    HMODULE runtime, party;
    void *handle = NULL;
    const char *text = NULL;
    HRESULT (WINAPI *initialize_runtime)(ULONG, ULONG);
    DWORD (WINAPI *initialize_party)(const char *, void **);
    DWORD (WINAPI *error_message)(DWORD, const char **);
    DWORD (WINAPI *cleanup)(void *);
    DWORD result;

    if (argc != 3) { fprintf(stderr, "Usage: %s <Windows DLL directory> <PlayFab title id>\n", argv[0]); return 2; }
    runtime = LoadLibraryA( "xgameruntime.dll" );
    initialize_runtime = runtime ? (void *)GetProcAddress(runtime, "InitializeApiImpl") : NULL;
    if (!initialize_runtime || FAILED(initialize_runtime(10002, 7822))) return 3;
    if (!SetDllDirectoryA(argv[1]) || !(party = LoadLibraryA("Party.dll"))) return 4;
    initialize_party = (void *)GetProcAddress(party, "PartyInitialize");
    error_message = (void *)GetProcAddress(party, "PartyGetErrorMessage");
    cleanup = (void *)GetProcAddress(party, "PartyCleanup");
    if (!initialize_party || !error_message || !cleanup) return 5;
    result = initialize_party(argv[2], &handle);
    error_message(result, &text);
    printf("PartyInitialize: %#lx (%s)\n", result, text ? text : "no error text");
    if (result) return 1;
    result = cleanup(handle);
    printf("PartyCleanup: %#lx\n", result);
    return result ? 1 : 0;
}
