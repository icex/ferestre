/*
 * A tracing stand-in for XCurl.dll.
 *
 * The title's HTTP goes through XCurl, which we already replace with a real
 * libcurl because Microsoft's WinHTTP-backed build never gets a request onto
 * the wire under Wine. That substitution left us blind: no URLs, no status
 * codes, so a failing request could only be guessed at.
 *
 * This forwards all sixteen entry points the title imports to the real
 * libcurl and writes a line per request: method, URL, status, and the reason
 * when a transfer fails. Header traffic is logged too, with credentials
 * redacted -- an Authorization header carries a live Xbox token and must not
 * reach a log file.
 *
 * Build (Proton SDK container):
 *   x86_64-w64-mingw32-gcc -O2 -shared -o XCurl.dll xcurl_trace.c
 *
 * Layout it expects in the game directory:
 *   XCurl.dll            this shim
 *   libcurl-x64.dll      the real libcurl
 *   XCurl.dll.microsoft  Microsoft's original, kept aside
 *
 * XCURL_TRACE_LOG names the log file; tracing is off when it is unset.
 *
 * Built with -DXCURL_REDACT_TEST and a native compiler, only the redaction
 * remains, with a main() that feeds it from stdin: that is how
 * tools/test_tool_hygiene.py checks that no credential reaches the log.
 */
#ifdef XCURL_REDACT_TEST
#include <stdio.h>
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>
#define __cdecl
#else
#include <windows.h>
#include <stdio.h>
#include <stdarg.h>
#include <string.h>
#endif

/* What a log line may say about a request without handing out its
 * credentials.
 *
 * Headers: any header whose name mentions a credential is reduced to its
 * length. Matching words inside the name rather than whole names is what
 * catches the ones nobody listed: PlayFab's X-Authorization and X-EntityToken,
 * a proxy's Proxy-Authorization, a title's own X-SecretKey.
 *
 * URLs: query strings carry credentials too (an Azure SAS signature, an
 * access_token parameter), so every value in one is reduced to its length,
 * keeping the parameter names for the reader. That applies to the request
 * line and to a Location header as much as to the URLs logged per request. */
static char ascii_lower( char c )
{
    return (c >= 'A' && c <= 'Z') ? (char)(c - 'A' + 'a') : c;
}

static int name_contains( const char *name, size_t len, const char *word )
{
    size_t k = strlen( word ), i, j;

    for (i = 0; i + k <= len; i++)
    {
        for (j = 0; j < k && ascii_lower( name[i + j] ) == word[j]; j++) ;
        if (j == k) return 1;
    }
    return 0;
}

/* The length of "Name:" when the line is a header whose value is a secret. */
static int header_is_secret( const char *line, size_t len )
{
    static const char *secret[] = {
        "auth", "token", "secret", "sign", "cookie", "key", "ticket",
        "session", "pass", "credential",
    };
    const char *colon = memchr( line, ':', len );
    size_t name_len, i;

    if (!colon) return 0;
    name_len = colon - line;
    /* A request or status line is not a header, even with a colon in it. */
    for (i = 0; i < name_len; i++) if (line[i] == ' ' || line[i] == '\t') return 0;
    for (i = 0; i < sizeof(secret)/sizeof(secret[0]); i++)
        if (name_contains( line, name_len, secret[i] )) return (int)(name_len + 1);
    return 0;
}

static int ends_query_value( char c )
{
    return c == '&' || c == '#' || c == ' ' || c == '\t';
}

/* Copy `in` to `out`, replacing every query value (and every fragment) with
 * its length in angle brackets. */
static void redact_query( const char *in, size_t len, char *out, size_t out_len )
{
    size_t i = 0, o = 0;

    if (!out_len) return;
    while (i < len && o + 1 < out_len)
    {
        if (in[i] != '?' && in[i] != '#')
        {
            out[o++] = in[i++];
            continue;
        }
        out[o++] = in[i++];
        /* name=value pairs up to the next space; a bare word counts as a value */
        while (i < len && in[i] != ' ' && in[i] != '\t' && o + 1 < out_len)
        {
            size_t start = i, value, n;
            char count[24];

            while (i < len && !ends_query_value( in[i] ) && in[i] != '=') i++;
            if (i < len && in[i] == '=')
            {
                n = i + 1 - start;
                if (o + n >= out_len) n = out_len - o - 1;
                memcpy( out + o, in + start, n );
                o += n;
                i++;
                value = i;
            }
            else value = start;
            while (i < len && !ends_query_value( in[i] )) i++;
            snprintf( count, sizeof(count), "<%u>", (unsigned)(i - value) );
            n = strlen( count );
            if (o + n >= out_len) n = out_len - o - 1;
            memcpy( out + o, count, n );
            o += n;
            if (i < len && (in[i] == '&' || in[i] == '#') && o + 1 < out_len) out[o++] = in[i++];
        }
    }
    out[o] = 0;
}

static void redact_url( const char *url, char *out, size_t out_len )
{
    redact_query( url, url ? strlen( url ) : 0, out, out_len );
}

#ifndef XCURL_REDACT_TEST

#define CURLOPT_URL             10002
#define CURLOPT_CUSTOMREQUEST   10036
#define CURLOPT_VERBOSE         41
#define CURLOPT_POST            47
#define CURLOPT_HTTPGET         80
#define CURLOPT_DEBUGFUNCTION   20094
#define CURLOPT_DEBUGDATA       10095
#define CURLINFO_RESPONSE_CODE  2097154   /* CURLINFO_LONG + 2 */
#define CURLINFO_EFFECTIVE_URL  1048577   /* CURLINFO_STRING + 1 */
#define CURLMSG_DONE            1

typedef struct { int msg; int _pad; void *easy_handle; void *result_union; } CURLMsg_x64;

static HMODULE real;
static FILE *logf;
static CRITICAL_SECTION log_cs;
static BOOL tracing;

/* One record per easy handle: what it was asked to fetch. */
#define MAX_HANDLES 512
static struct { void *h; char method[16]; char url[512]; } handles[MAX_HANDLES];

static void note_handle( void *h, const char *method, const char *url )
{
    int i, free_slot = -1;

    EnterCriticalSection( &log_cs );
    for (i = 0; i < MAX_HANDLES; i++)
    {
        if (handles[i].h == h) break;
        if (!handles[i].h && free_slot < 0) free_slot = i;
    }
    if (i == MAX_HANDLES) i = free_slot;
    if (i >= 0)
    {
        handles[i].h = h;
        if (method) { strncpy( handles[i].method, method, sizeof(handles[i].method) - 1 ); handles[i].method[sizeof(handles[i].method)-1] = 0; }
        if (url) redact_url( url, handles[i].url, sizeof(handles[i].url) );
    }
    LeaveCriticalSection( &log_cs );
}

static void forget_handle( void *h )
{
    int i;
    EnterCriticalSection( &log_cs );
    for (i = 0; i < MAX_HANDLES; i++) if (handles[i].h == h) { memset( &handles[i], 0, sizeof(handles[i]) ); break; }
    LeaveCriticalSection( &log_cs );
}

static void describe( void *h, char *method, size_t mlen, char *url, size_t ulen )
{
    int i;
    method[0] = url[0] = 0;
    EnterCriticalSection( &log_cs );
    for (i = 0; i < MAX_HANDLES; i++)
        if (handles[i].h == h)
        {
            strncpy( method, handles[i].method[0] ? handles[i].method : "GET", mlen - 1 ); method[mlen-1] = 0;
            strncpy( url, handles[i].url, ulen - 1 ); url[ulen-1] = 0;
            break;
        }
    LeaveCriticalSection( &log_cs );
}

static void trace( const char *fmt, ... )
{
    SYSTEMTIME t;
    va_list ap;

    if (!tracing || !logf) return;
    GetLocalTime( &t );
    EnterCriticalSection( &log_cs );
    fprintf( logf, "%02d:%02d:%02d.%03d ", t.wHour, t.wMinute, t.wSecond, t.wMilliseconds );
    va_start( ap, fmt );
    vfprintf( logf, fmt, ap );
    va_end( ap );
    fputc( '\n', logf );
    fflush( logf );
    LeaveCriticalSection( &log_cs );
}

#else

static void trace( const char *fmt, ... )
{
    va_list ap;

    va_start( ap, fmt );
    vprintf( fmt, ap );
    va_end( ap );
    putchar( '\n' );
}

#endif

/* Header traffic, with anything bearing a credential reduced to its length.
 *
 * libcurl hands the whole header block to this callback in one piece, not a
 * line at a time, so the redaction has to walk the block line by line. Getting
 * that wrong writes live Xbox tokens to a file, which is exactly what an
 * earlier version of this did. */

static int __cdecl debug_cb( void *handle, int type, char *data, size_t size, void *user )
{
    const char *p, *end;

    if (type != 1 && type != 2) return 0;   /* headers in / out only */

    p = data;
    end = data + size;
    while (p < end)
    {
        const char *nl = memchr( p, '\n', end - p );
        size_t len = (nl ? (size_t)(nl - p) : (size_t)(end - p));
        int k;

        while (len && (p[len-1] == '\r' || p[len-1] == '\n')) len--;
        if (len)
        {
            char line[600];
            size_t n = len < sizeof(line) - 1 ? len : sizeof(line) - 1;

            memcpy( line, p, n );
            line[n] = 0;
            if ((k = header_is_secret( line, n )))
                trace( "  %s %.*s <redacted, %u bytes>", type == 2 ? ">" : "<",
                       k, line, (unsigned)(n - k) );
            else
            {
                char shown[600];

                redact_query( line, n, shown, sizeof(shown) );
                trace( "  %s %s", type == 2 ? ">" : "<", shown );
            }
        }
        if (!nl) break;
        p = nl + 1;
    }
    return 0;
}

#ifndef XCURL_REDACT_TEST

/* Percent-encode the characters libcurl refuses in a URL.
 *
 * Microsoft's XCurl accepts them; stock libcurl answers CURLE_URL_MALFORMAT
 * and the transfer never starts. Minecraft's asset CDN serves names with
 * spaces and brackets in them ("logomark_48x48 (1).png"), so without this the
 * title silently loses images and other content. Anything already
 * percent-encoded is left alone so a valid URL is never double-encoded. */
static const char *url_fixup( const char *url, char *out, size_t out_len )
{
    static const char *bad = " \"<>\\^`{|}";
    const unsigned char *p = (const unsigned char *)url;
    size_t o = 0;

    if (!url) return url;
    if (!strpbrk( url, bad )) return url;      /* nothing to do */

    for (; *p && o + 4 < out_len; p++)
    {
        if (*p < 0x21 || *p > 0x7e || strchr( bad, *p ))
        {
            static const char hex[] = "0123456789ABCDEF";
            out[o++] = '%';
            out[o++] = hex[*p >> 4];
            out[o++] = hex[*p & 15];
        }
        else out[o++] = (char)*p;
    }
    out[o] = 0;
    return out;
}

#define REAL(name) ((void *)GetProcAddress( real, name ))

__declspec(dllexport) void *curl_easy_init( void )
{
    void *(*fn)(void) = REAL("curl_easy_init");
    void *h = fn ? fn() : NULL;
    if (h && tracing)
    {
        int (*setopt)(void *, int, ...) = REAL("curl_easy_setopt");
        if (setopt)
        {
            setopt( h, CURLOPT_VERBOSE, (long)1 );
            setopt( h, CURLOPT_DEBUGFUNCTION, debug_cb );
        }
        note_handle( h, "GET", "" );
    }
    return h;
}

__declspec(dllexport) int curl_easy_setopt( void *h, int opt, ... )
{
    int (*fn)(void *, int, void *) = REAL("curl_easy_setopt");
    va_list ap;
    void *val;

    va_start( ap, opt );
    val = va_arg( ap, void * );
    va_end( ap );

    /* Keep our own tracing hooks in place if the caller sets its own. */
    if (tracing && (opt == CURLOPT_DEBUGFUNCTION || opt == CURLOPT_VERBOSE)) return 0;

    if (opt == CURLOPT_URL && val)
    {
        /* A plain local: thread-local storage in a DLL loaded with
         * LoadLibrary is not reliably initialised, and libcurl copies the URL
         * string, so the buffer only has to outlive the call below. */
        char fixed[2048];
        const char *clean = url_fixup( (const char *)val, fixed, sizeof(fixed) );

        if (clean != (const char *)val)
        {
            char shown[512];

            redact_url( (const char *)val, shown, sizeof(shown) );
            trace( "  ! url re-encoded: %s", shown );
        }
        val = (void *)clean;
        if (tracing) note_handle( h, NULL, clean );
    }
    if (tracing && opt == CURLOPT_CUSTOMREQUEST && val) note_handle( h, (const char *)val, NULL );
    if (tracing && opt == CURLOPT_POST && val) note_handle( h, "POST", NULL );
    if (tracing && opt == CURLOPT_HTTPGET && val) note_handle( h, "GET", NULL );

    return fn ? fn( h, opt, val ) : -1;
}

__declspec(dllexport) void curl_easy_cleanup( void *h )
{
    void (*fn)(void *) = REAL("curl_easy_cleanup");
    forget_handle( h );
    if (fn) fn( h );
}

__declspec(dllexport) int curl_easy_getinfo( void *h, int info, ... )
{
    int (*fn)(void *, int, void *) = REAL("curl_easy_getinfo");
    va_list ap; void *out;
    va_start( ap, info ); out = va_arg( ap, void * ); va_end( ap );
    return fn ? fn( h, info, out ) : -1;
}

__declspec(dllexport) const char *curl_easy_strerror( int code )
{
    const char *(*fn)(int) = REAL("curl_easy_strerror");
    return fn ? fn( code ) : "";
}

__declspec(dllexport) int curl_global_init( long flags )
{
    int (*fn)(long) = REAL("curl_global_init");
    trace( "curl_global_init(0x%lx)", flags );
    return fn ? fn( flags ) : -1;
}

__declspec(dllexport) void curl_global_cleanup( void )
{
    void (*fn)(void) = REAL("curl_global_cleanup");
    trace( "curl_global_cleanup" );
    if (fn) fn();
}

__declspec(dllexport) void *curl_multi_init( void )
{
    void *(*fn)(void) = REAL("curl_multi_init");
    return fn ? fn() : NULL;
}

__declspec(dllexport) int curl_multi_add_handle( void *m, void *e )
{
    int (*fn)(void *, void *) = REAL("curl_multi_add_handle");
    char method[16], url[512];
    describe( e, method, sizeof(method), url, sizeof(url) );
    trace( "--> %s %s", method, url[0] ? url : "(no url set)" );
    return fn ? fn( m, e ) : -1;
}

__declspec(dllexport) int curl_multi_remove_handle( void *m, void *e )
{
    int (*fn)(void *, void *) = REAL("curl_multi_remove_handle");
    return fn ? fn( m, e ) : -1;
}

__declspec(dllexport) int curl_multi_perform( void *m, int *running )
{
    int (*fn)(void *, int *) = REAL("curl_multi_perform");
    return fn ? fn( m, running ) : -1;
}

__declspec(dllexport) int curl_multi_poll( void *m, void *fds, unsigned int nfds, int timeout, int *numfds )
{
    int (*fn)(void *, void *, unsigned int, int, int *) = REAL("curl_multi_poll");
    return fn ? fn( m, fds, nfds, timeout, numfds ) : -1;
}

__declspec(dllexport) void *curl_multi_info_read( void *m, int *msgs_in_queue )
{
    void *(*fn)(void *, int *) = REAL("curl_multi_info_read");
    CURLMsg_x64 *msg = fn ? fn( m, msgs_in_queue ) : NULL;

    if (msg && tracing && msg->msg == CURLMSG_DONE)
    {
        int (*getinfo)(void *, int, void *) = REAL("curl_easy_getinfo");
        const char *(*strerror_fn)(int) = REAL("curl_easy_strerror");
        int result = (int)(ULONG_PTR)msg->result_union;
        char method[16], url[512];
        long status = 0;

        describe( msg->easy_handle, method, sizeof(method), url, sizeof(url) );
        if (getinfo) getinfo( msg->easy_handle, CURLINFO_RESPONSE_CODE, &status );
        if (result)
            trace( "<-- %s %s FAILED curl=%d (%s)", method, url, result,
                   strerror_fn ? strerror_fn( result ) : "?" );
        else
            trace( "<-- %s %s HTTP %ld", method, url, status );
    }
    return msg;
}

__declspec(dllexport) int curl_multi_cleanup( void *m )
{
    int (*fn)(void *) = REAL("curl_multi_cleanup");
    return fn ? fn( m ) : -1;
}

__declspec(dllexport) void *curl_slist_append( void *list, const char *s )
{
    void *(*fn)(void *, const char *) = REAL("curl_slist_append");
    return fn ? fn( list, s ) : NULL;
}

__declspec(dllexport) void curl_slist_free_all( void *list )
{
    void (*fn)(void *) = REAL("curl_slist_free_all");
    if (fn) fn( list );
}

BOOL WINAPI DllMain( HINSTANCE inst, DWORD reason, void *reserved )
{
    char path[MAX_PATH], *slash;
    const char *log_path;

    if (reason != DLL_PROCESS_ATTACH) return TRUE;
    DisableThreadLibraryCalls( inst );
    InitializeCriticalSection( &log_cs );

    /* Load the real libcurl from beside this DLL. */
    if (GetModuleFileNameA( inst, path, sizeof(path) ) &&
        (slash = strrchr( path, '\\' )))
    {
        strcpy( slash + 1, "libcurl-x64.dll" );
        real = LoadLibraryA( path );
    }
    if (!real) real = LoadLibraryA( "libcurl-x64.dll" );

    if ((log_path = getenv( "XCURL_TRACE_LOG" )) && *log_path)
    {
        logf = fopen( log_path, "a" );
        tracing = logf != NULL;
        trace( "xcurl trace attached, real libcurl %s", real ? "loaded" : "MISSING" );
    }
    return real != NULL;
}

#else

/* The redaction on its own: stdin is a header block as libcurl hands it to
 * the debug callback, and each argument is a URL to log. */
int main( int argc, char **argv )
{
    static char block[65536];
    size_t size = fread( block, 1, sizeof(block), stdin );
    int i;

    debug_cb( NULL, 2, block, size, NULL );
    for (i = 1; i < argc; i++)
    {
        char shown[512];

        redact_url( argv[i], shown, sizeof(shown) );
        trace( "--> GET %s", shown );
    }
    return 0;
}

#endif
