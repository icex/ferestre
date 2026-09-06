/*
 * Can this stack open a WebSocket the way libHttpClient does?
 *
 * Xbox Live's real-time activity channel is a WebSocket, and on the GDK
 * libHttpClient opens it through WinHTTP -- not through XCurl, so none of it
 * appears in the HTTP trace. A multiplayer join that completes every HTTP call
 * and then sits waiting is exactly what a dead notification channel looks
 * like, so this checks the capability directly.
 *
 *   wss_probe.exe <host> <path>
 *
 * Build: x86_64-w64-mingw32-gcc -O2 -o wss_probe.exe wss_probe.c -lwinhttp
 */
#include <windows.h>
#include <winhttp.h>
#include <stdio.h>

int main( int argc, char **argv )
{
    const char *host_a = argc > 1 ? argv[1] : "rta.xboxlive.com";
    const char *path_a = argc > 2 ? argv[2] : "/connect";
    WCHAR host[256], path[256];
    HINTERNET session, connect, request, socket;
    DWORD err;

    MultiByteToWideChar( CP_ACP, 0, host_a, -1, host, 256 );
    MultiByteToWideChar( CP_ACP, 0, path_a, -1, path, 256 );

    session = WinHttpOpen( L"wss_probe/1.0", WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                           WINHTTP_NO_PROXY_NAME, WINHTTP_NO_PROXY_BYPASS, 0 );
    if (!session) { printf( "WinHttpOpen failed %lu\n", GetLastError() ); return 2; }

    connect = WinHttpConnect( session, host, INTERNET_DEFAULT_HTTPS_PORT, 0 );
    if (!connect) { printf( "WinHttpConnect failed %lu\n", GetLastError() ); return 2; }

    request = WinHttpOpenRequest( connect, L"GET", path, NULL, WINHTTP_NO_REFERER,
                                  WINHTTP_DEFAULT_ACCEPT_TYPES, WINHTTP_FLAG_SECURE );
    if (!request) { printf( "WinHttpOpenRequest failed %lu\n", GetLastError() ); return 2; }

    /* Ask for the upgrade the same way libHttpClient does. */
    if (!WinHttpSetOption( request, WINHTTP_OPTION_UPGRADE_TO_WEB_SOCKET, NULL, 0 ))
    {
        err = GetLastError();
        printf( "UPGRADE_TO_WEB_SOCKET option REJECTED, error %lu%s\n", err,
                err == ERROR_WINHTTP_INVALID_OPTION ? "  (option not supported here)" : "" );
        return 3;
    }

    if (!WinHttpSendRequest( request, WINHTTP_NO_ADDITIONAL_HEADERS, 0,
                             WINHTTP_NO_REQUEST_DATA, 0, 0, 0 ))
    { printf( "WinHttpSendRequest failed %lu\n", GetLastError() ); return 2; }

    if (!WinHttpReceiveResponse( request, NULL ))
    { printf( "WinHttpReceiveResponse failed %lu\n", GetLastError() ); return 2; }

    socket = WinHttpWebSocketCompleteUpgrade( request, 0 );
    if (!socket)
    {
        DWORD status = 0, len = sizeof(status);
        WinHttpQueryHeaders( request, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                             NULL, &status, &len, NULL );
        printf( "CompleteUpgrade failed %lu (HTTP status %lu)\n", GetLastError(), status );
        /* 401 here is expected without a token: it still proves the machinery works */
        return status == 401 ? 0 : 4;
    }

    printf( "WebSocket upgrade SUCCEEDED\n" );
    WinHttpWebSocketClose( socket, WINHTTP_WEB_SOCKET_SUCCESS_CLOSE_STATUS, NULL, 0 );
    return 0;
}
