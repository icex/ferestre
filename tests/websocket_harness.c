/*
 * Drive libHttpClient's WebSocket lifecycle directly, without the game.
 *
 * Minecraft's server-list join opens two WebSockets (Xbox Live's Real-Time
 * Activity socket and the multiplayer signalling socket), and something on that
 * path leaves an HC_WEBSOCKET_OBSERVER alive after its xbox::httpclient::WebSocket
 * has been destroyed: the observer's destructor then locks the WebSocket's mutex
 * at +0x108 in freed memory. Depending on what the freed block has been reused
 * for, that either faults (the intermittent read of address 0x8) or parks for
 * ever on a lock nobody will release -- and since the thread is holding a
 * Minecraft mutex at the time, the game's main thread blocks behind it and the
 * whole title freezes.
 *
 * Reproducing that through the game takes minutes and a person clicking through
 * menus. libHttpClient is an ordinary DLL with a documented C API, so this
 * drives the same lifecycle against tests/ws_server.py in a couple of seconds.
 *
 * Usage: websocket_harness.exe <ws-uri> <scenario> [iterations]
 */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>

#include "xasyncprovider.h"     /* XAsyncBlock */

typedef struct HC_WEBSOCKET *HCWebsocketHandle;

typedef struct
{
    HCWebsocketHandle websocket;
    HRESULT errorCode;
    UINT32 platformErrorCode;
} WebSocketCompletionResult;

typedef void (CALLBACK *HCWebSocketMessageFunction)( HCWebsocketHandle, const char *, void * );
typedef void (CALLBACK *HCWebSocketBinaryMessageFunction)( HCWebsocketHandle, const UINT8 *, UINT32, void * );
typedef void (CALLBACK *HCWebSocketCloseEventFunction)( HCWebsocketHandle, UINT32, void * );
typedef void (CALLBACK *HCTraceCallback)( const char *, UINT32, UINT64, UINT64, const char * );

static HRESULT (WINAPI *pHCInitialize)( void * );
static void    (WINAPI *pHCCleanup)( void );
static HRESULT (WINAPI *pHCSettingsSetTraceLevel)( UINT32 );
static void    (WINAPI *pHCTraceSetClientCallback)( HCTraceCallback );
static void    (WINAPI *pHCTraceSetTraceToDebugger)( BOOL );
static HRESULT (WINAPI *pHCWebSocketCreate)( HCWebsocketHandle *, HCWebSocketMessageFunction,
                                             HCWebSocketBinaryMessageFunction, HCWebSocketCloseEventFunction, void * );
static HRESULT (WINAPI *pHCWebSocketConnectAsync)( const char *, const char *, HCWebsocketHandle, XAsyncBlock * );
static HRESULT (WINAPI *pHCGetWebSocketConnectResult)( XAsyncBlock *, WebSocketCompletionResult * );
static HRESULT (WINAPI *pHCWebSocketDisconnect)( HCWebsocketHandle );
static void    (WINAPI *pHCWebSocketCloseHandle)( HCWebsocketHandle );
static HCWebsocketHandle (WINAPI *pHCWebSocketDuplicateHandle)( HCWebsocketHandle );

static volatile LONG connects_done, closes_seen, messages_seen;
static volatile LONG watchdog_tripped;
static const char *g_stage = "starting";

static void CALLBACK trace_cb( const char *area, UINT32 level, UINT64 thread, UINT64 stamp, const char *message )
{
    printf( "    [hc] %s\n", message ? message : "(null)" );
    fflush( stdout );
}

static void CALLBACK on_message( HCWebsocketHandle ws, const char *text, void *ctx )
{
    InterlockedIncrement( &messages_seen );
}

static void CALLBACK on_binary( HCWebsocketHandle ws, const UINT8 *bytes, UINT32 size, void *ctx )
{
    InterlockedIncrement( &messages_seen );
}

static void CALLBACK on_close( HCWebsocketHandle ws, UINT32 status, void *ctx )
{
    InterlockedIncrement( &closes_seen );
}

/* The failure this chases is a hang, so the harness has to time itself out. */
static DWORD WINAPI watchdog( void *arg )
{
    DWORD seconds = (DWORD)(ULONG_PTR)arg;

    Sleep( seconds * 1000 );
    InterlockedExchange( &watchdog_tripped, 1 );
    printf( "FAIL - stuck in stage '%s' after %lu seconds\n", g_stage, seconds );
    fflush( stdout );
    ExitProcess( 2 );
    return 0;
}

/* libHttpClient gates WebSocket connects on the GDK reporting an initialised
 * network (XNetworkingGetConnectivityHint().networkInitialized), so the runtime
 * has to be up first or every connect fails before it reaches the wire. The
 * public XGameRuntimeInitialize lives in the GDK's static library, so call the
 * DLL export it forwards to. */
static BOOL init_gameruntime( void )
{
    HRESULT (WINAPI *initialize)( ULONG, ULONG );
    HMODULE mod = LoadLibraryA( "xgameruntime.dll" );
    HRESULT hr;

    if (!mod) { printf( "Bail out! xgameruntime.dll: %lu\n", GetLastError() ); return FALSE; }
    if (!(initialize = (void *)GetProcAddress( mod, "InitializeApiImpl" )))
    {
        printf( "Bail out! xgameruntime.dll has no InitializeApiImpl\n" );
        return FALSE;
    }
    if (FAILED(hr = initialize( 0x0000ffff, 0x0000ffff )))
    {
        printf( "Bail out! InitializeApiImpl 0x%08lx\n", hr );
        return FALSE;
    }
    return TRUE;
}

static BOOL load_libhttpclient( void )
{
    HMODULE mod = LoadLibraryA( "libHttpClient.GDK.dll" );

    if (!mod) { printf( "Bail out! libHttpClient.GDK.dll: %lu\n", GetLastError() ); return FALSE; }

#define GET(f) do { *(FARPROC *)&p##f = GetProcAddress( mod, #f ); \
    if (!p##f) { printf( "Bail out! missing export %s\n", #f ); return FALSE; } } while (0)
    GET(HCInitialize);
    GET(HCCleanup);
    GET(HCWebSocketCreate);
    GET(HCWebSocketConnectAsync);
    GET(HCGetWebSocketConnectResult);
    GET(HCWebSocketDisconnect);
    GET(HCWebSocketCloseHandle);
#undef GET
    /* Optional: tracing and handle duplication. */
    *(FARPROC *)&pHCSettingsSetTraceLevel = GetProcAddress( mod, "HCSettingsSetTraceLevel" );
    *(FARPROC *)&pHCTraceSetClientCallback = GetProcAddress( mod, "HCTraceSetClientCallback" );
    *(FARPROC *)&pHCTraceSetTraceToDebugger = GetProcAddress( mod, "HCTraceSetTraceToDebugger" );
    *(FARPROC *)&pHCWebSocketDuplicateHandle = GetProcAddress( mod, "HCWebSocketDuplicateHandle" );
    return TRUE;
}

/* Wait for an async block, without pumping anything: the runtime delivers
 * completions on its own threads. */
static HRESULT wait_for( XAsyncBlock *async, DWORD ms )
{
    DWORD waited;

    for (waited = 0; waited < ms; waited += 5)
    {
        WebSocketCompletionResult result = { 0 };
        HRESULT hr = pHCGetWebSocketConnectResult( async, &result );

        if (hr != E_PENDING) return hr;
        Sleep( 5 );
    }
    return E_PENDING;
}

static int run_once( const char *uri, const char *scenario )
{
    HCWebsocketHandle ws = NULL;
    XAsyncBlock async = { 0 };
    HRESULT hr;

    g_stage = "create";
    if (FAILED(hr = pHCWebSocketCreate( &ws, on_message, on_binary, on_close, NULL )))
    {
        printf( "    create failed 0x%08lx\n", hr );
        return 1;
    }

    g_stage = "connect";
    hr = pHCWebSocketConnectAsync( uri, "", ws, &async );
    if (FAILED(hr))
    {
        printf( "    connect submit failed 0x%08lx\n", hr );
        pHCWebSocketCloseHandle( ws );
        return 1;
    }

    if (!strcmp( scenario, "close-during-connect" ))
    {
        /* Close the handle while the connect is still in flight. This is the
         * shape the game hits: the title drops its reference as the connect is
         * failing. */
        Sleep( 1 );
        g_stage = "close-during-connect";
        pHCWebSocketCloseHandle( ws );
        wait_for( &async, 5000 );
        return 0;
    }

    g_stage = "await-connect";
    hr = wait_for( &async, 10000 );
    InterlockedIncrement( &connects_done );

    if (!strcmp( scenario, "disconnect-then-close" ))
    {
        g_stage = "disconnect";
        pHCWebSocketDisconnect( ws );
        Sleep( 50 );
    }
    else if (!strcmp( scenario, "dup-close" ) && pHCWebSocketDuplicateHandle)
    {
        HCWebsocketHandle dup = pHCWebSocketDuplicateHandle( ws );

        g_stage = "dup-close";
        pHCWebSocketCloseHandle( ws );
        if (dup) pHCWebSocketCloseHandle( dup );
        return 0;
    }

    g_stage = "close";
    pHCWebSocketCloseHandle( ws );
    return 0;
}

int main( int argc, char **argv )
{
    const char *uri = argc > 1 ? argv[1] : "ws://127.0.0.1:18080/";
    const char *scenario = argc > 2 ? argv[2] : "connect-close";
    int iterations = argc > 3 ? atoi( argv[3] ) : 1;
    HRESULT hr;
    int i;

    setvbuf( stdout, NULL, _IONBF, 0 );
    if (!init_gameruntime()) return 1;
    if (!load_libhttpclient()) return 1;

    CloseHandle( CreateThread( NULL, 0, watchdog, (void *)(ULONG_PTR)60, 0, NULL ) );

    if (pHCTraceSetClientCallback && pHCSettingsSetTraceLevel && getenv( "HC_TRACE" ))
    {
        pHCTraceSetClientCallback( trace_cb );
        pHCSettingsSetTraceLevel( 5 );   /* Verbose */
    }

    g_stage = "HCInitialize";
    if (FAILED(hr = pHCInitialize( NULL )))
    {
        printf( "Bail out! HCInitialize 0x%08lx\n", hr );
        return 1;
    }

    printf( "# %s against %s, %d iteration(s)\n", scenario, uri, iterations );
    for (i = 0; i < iterations; i++)
    {
        printf( "  iteration %d\n", i + 1 );
        run_once( uri, scenario );
    }

    g_stage = "HCCleanup";
    printf( "  HCCleanup\n" );
    pHCCleanup();

    g_stage = "done";
    printf( "ok - %s survived: %ld connect(s), %ld close event(s), %ld message(s)\n",
            scenario, connects_done, closes_seen, messages_seen );
    return 0;
}
