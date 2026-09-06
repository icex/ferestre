/*
 * Exercise the XGameSave implementation in xgameruntime.dll without a game.
 *
 * Talks to the DLL the way the GDK static library does: InitializeApiImpl,
 * then QueryApiImpl for the XUser, XThreading and XGameSave interfaces and
 * calls through their vtables.
 *
 *   xgstest <scid> list
 *   xgstest <scid> read <container> [blob...]
 *   xgstest <scid> readasync <container>
 *   xgstest <scid> write <container> <blob> <file> [display name]
 *   xgstest <scid> writeasync <container> <blob> <file> [display name]
 *   xgstest <scid> delblob <container> <blob>
 *   xgstest <scid> delete <container>
 *   xgstest <scid> quota
 *   xgstest <scid> files
 *
 * Build (inside the Proton SDK container, mingw headers plus the generated
 * interface headers from the Wine build tree):
 *   x86_64-w64-mingw32-gcc -o xgstest.exe xgstest.c -D__WINESRC__ -DCOBJMACROS \
 *       -I<obj>/dlls/xgameruntime -I<src>/dlls/xgameruntime
 */

#define COBJMACROS
#include <windows.h>
#include <initguid.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "xgamesave.h"
#include "xuser.h"
#include "xasyncprovider.h"

#define E_GS_PROVIDED_BUFFER_TOO_SMALL ((HRESULT)0x80830007L)

typedef HRESULT (WINAPI *InitializeApiImpl_t)( ULONG, ULONG );
typedef HRESULT (WINAPI *QueryApiImpl_t)( REFCLSID, REFIID, void ** );

static IXGameSaveImpl3 *gs;
static IXThreadingImpl *threading;
static XUserHandle user;

struct blob_sizes
{
    unsigned int count;
    size_t data;
    size_t names;
};

static BOOLEAN __stdcall on_container( const XGameSaveContainerInfo *info, void *ctx )
{
    printf( "container '%s'  display='%s'  blobs=%u  size=%llu  mtime=%llu\n", info->name, info->displayName,
            info->blobCount, (unsigned long long)info->totalSize, (unsigned long long)info->lastModifiedTime );
    return TRUE;
}

static BOOLEAN __stdcall on_blob( const XGameSaveBlobInfo *info, void *ctx )
{
    struct blob_sizes *sizes = ctx;

    printf( "  blob '%s'  size=%u\n", info->name, info->size );
    sizes->count++;
    sizes->data += info->size;
    sizes->names += strlen( info->name ) + 1;
    return TRUE;
}

static void dump_blobs( XGameSaveBlob *blobs, unsigned int count )
{
    unsigned int i, j;

    for (i = 0; i < count; i++)
    {
        unsigned int sum = 0;

        for (j = 0; j < blobs[i].info.size; j++) sum = sum * 31 + blobs[i].data[j];
        printf( "  read '%s'  size=%u  hash=%08x  head=", blobs[i].info.name, blobs[i].info.size, sum );
        for (j = 0; j < 8 && j < blobs[i].info.size; j++) printf( "%02x", blobs[i].data[j] );
        printf( "\n" );
    }
}

static BYTE *read_file( const char *path, size_t *size )
{
    FILE *f = fopen( path, "rb" );
    BYTE *data;
    long len;

    if (!f) return NULL;
    fseek( f, 0, SEEK_END );
    len = ftell( f );
    fseek( f, 0, SEEK_SET );
    data = malloc( len ? len : 1 );
    *size = fread( data, 1, len, f );
    fclose( f );
    return data;
}

static int cmd_list( XGameSaveProviderHandle provider )
{
    HRESULT hr = IXGameSaveImpl3_XGameSaveEnumerateContainerInfo( gs, provider, NULL, on_container );
    printf( "EnumerateContainerInfo: 0x%08lx\n", hr );
    return FAILED( hr );
}

static int cmd_blobs( XGameSaveProviderHandle provider, const char *name )
{
    XGameSaveContainerHandle container;
    struct blob_sizes sizes = { 0 };
    HRESULT hr;

    hr = IXGameSaveImpl3_XGameSaveCreateContainer( gs, provider, name, &container );
    printf( "CreateContainer('%s'): 0x%08lx\n", name, hr );
    if (FAILED( hr )) return 1;
    hr = IXGameSaveImpl3_XGameSaveEnumerateBlobInfo( gs, container, &sizes, on_blob );
    printf( "EnumerateBlobInfo: 0x%08lx, %u blobs, %zu bytes\n", hr, sizes.count, sizes.data );
    IXGameSaveImpl3_XGameSaveCloseContainer( gs, container );
    return FAILED( hr );
}

static int cmd_read( XGameSaveProviderHandle provider, const char *name, const char **names, unsigned int count )
{
    XGameSaveContainerHandle container;
    struct blob_sizes sizes = { 0 };
    XGameSaveBlob *blobs;
    size_t alloc;
    HRESULT hr;

    hr = IXGameSaveImpl3_XGameSaveCreateContainer( gs, provider, name, &container );
    printf( "CreateContainer('%s'): 0x%08lx\n", name, hr );
    if (FAILED( hr )) return 1;
    hr = IXGameSaveImpl3_XGameSaveEnumerateBlobInfo( gs, container, &sizes, on_blob );
    printf( "EnumerateBlobInfo: 0x%08lx\n", hr );

    /* what the documentation's sample sizes the buffer to */
    alloc = sizes.count * sizeof(XGameSaveBlob) + sizes.data + sizes.names;
    blobs = malloc( alloc ? alloc : 1 );
    if (!count) count = sizes.count;
    hr = IXGameSaveImpl3_XGameSaveReadBlobData( gs, container, names, &count, alloc, blobs );
    printf( "ReadBlobData: 0x%08lx, count=%u (buffer %zu)\n", hr, count, alloc );
    if (SUCCEEDED( hr )) dump_blobs( blobs, count );

    /* too small on purpose: must fail cleanly and report the count */
    if (sizes.data)
    {
        unsigned int n = 0;
        hr = IXGameSaveImpl3_XGameSaveReadBlobData( gs, container, NULL, &n, alloc / 2, blobs );
        printf( "ReadBlobData(half buffer): 0x%08lx, count=%u (%s)\n", hr, n,
                hr == E_GS_PROVIDED_BUFFER_TOO_SMALL ? "expected" : "UNEXPECTED" );
    }
    free( blobs );
    IXGameSaveImpl3_XGameSaveCloseContainer( gs, container );
    return 0;
}

static int cmd_read_async( XGameSaveProviderHandle provider, const char *name )
{
    XGameSaveContainerHandle container;
    XAsyncBlock async = { 0 };
    XGameSaveBlob *blobs;
    SIZE_T size = 0;
    unsigned int count = 0;
    HRESULT hr;

    hr = IXGameSaveImpl3_XGameSaveCreateContainer( gs, provider, name, &container );
    if (FAILED( hr )) return 1;
    hr = IXGameSaveImpl3_XGameSaveReadBlobDataAsync( gs, container, NULL, 0, &async );
    printf( "ReadBlobDataAsync: 0x%08lx\n", hr );
    hr = IXThreadingImpl_XAsyncGetResultSize( threading, &async, &size );
    printf( "XAsyncGetResultSize: 0x%08lx, size=%zu\n", hr, (size_t)size );
    blobs = malloc( size ? size : 1 );
    hr = IXGameSaveImpl3_XGameSaveReadBlobDataResult( gs, &async, size, blobs, &count );
    printf( "ReadBlobDataResult: 0x%08lx, count=%u\n", hr, count );
    if (SUCCEEDED( hr )) dump_blobs( blobs, count );
    free( blobs );
    IXGameSaveImpl3_XGameSaveCloseContainer( gs, container );
    return FAILED( hr );
}

static int cmd_write( XGameSaveProviderHandle provider, const char *name, const char *blob, const char *file,
                      const char *display, BOOL use_async )
{
    XGameSaveContainerHandle container;
    XGameSaveUpdateHandle update;
    BYTE *data;
    size_t size;
    HRESULT hr;

    if (!(data = read_file( file, &size )))
    {
        printf( "cannot read %s\n", file );
        return 1;
    }
    hr = IXGameSaveImpl3_XGameSaveCreateContainer( gs, provider, name, &container );
    printf( "CreateContainer('%s'): 0x%08lx\n", name, hr );
    if (FAILED( hr )) return 1;
    hr = IXGameSaveImpl3_XGameSaveCreateUpdate( gs, container, display, &update );
    printf( "CreateUpdate(display='%s'): 0x%08lx\n", display ? display : "(null)", hr );
    hr = IXGameSaveImpl3_XGameSaveSubmitBlobWrite( gs, update, blob, data, size );
    printf( "SubmitBlobWrite('%s', %zu bytes): 0x%08lx\n", blob, size, hr );
    if (use_async)
    {
        XAsyncBlock async = { 0 };
        hr = IXGameSaveImpl3_XGameSaveSubmitUpdateAsync( gs, update, &async );
        printf( "SubmitUpdateAsync: 0x%08lx\n", hr );
        hr = IXGameSaveImpl3_XGameSaveSubmitUpdateResult( gs, &async );
        printf( "SubmitUpdateResult: 0x%08lx\n", hr );
    }
    else
    {
        hr = IXGameSaveImpl3_XGameSaveSubmitUpdate( gs, update );
        printf( "SubmitUpdate: 0x%08lx\n", hr );
    }
    IXGameSaveImpl3_XGameSaveCloseUpdate( gs, update );
    IXGameSaveImpl3_XGameSaveCloseContainer( gs, container );
    free( data );
    return FAILED( hr );
}

static int cmd_delblob( XGameSaveProviderHandle provider, const char *name, const char *blob )
{
    XGameSaveContainerHandle container;
    XGameSaveUpdateHandle update;
    HRESULT hr;

    hr = IXGameSaveImpl3_XGameSaveCreateContainer( gs, provider, name, &container );
    if (FAILED( hr )) return 1;
    hr = IXGameSaveImpl3_XGameSaveCreateUpdate( gs, container, NULL, &update );
    hr = IXGameSaveImpl3_XGameSaveSubmitBlobDelete( gs, update, blob );
    printf( "SubmitBlobDelete('%s'): 0x%08lx\n", blob, hr );
    hr = IXGameSaveImpl3_XGameSaveSubmitUpdate( gs, update );
    printf( "SubmitUpdate: 0x%08lx\n", hr );
    IXGameSaveImpl3_XGameSaveCloseUpdate( gs, update );
    IXGameSaveImpl3_XGameSaveCloseContainer( gs, container );
    return FAILED( hr );
}

static int cmd_delete( XGameSaveProviderHandle provider, const char *name )
{
    XAsyncBlock async = { 0 };
    HRESULT hr;

    hr = IXGameSaveImpl3_XGameSaveDeleteContainerAsync( gs, provider, name, &async );
    printf( "DeleteContainerAsync('%s'): 0x%08lx\n", name, hr );
    hr = IXGameSaveImpl3_XGameSaveDeleteContainerResult( gs, &async );
    printf( "DeleteContainerResult: 0x%08lx\n", hr );
    return FAILED( hr );
}

static int cmd_quota( XGameSaveProviderHandle provider )
{
    XAsyncBlock async = { 0 };
    INT64 remaining = -1;
    HRESULT hr;

    hr = IXGameSaveImpl3_XGameSaveGetRemainingQuota( gs, provider, &remaining );
    printf( "GetRemainingQuota: 0x%08lx, %lld bytes\n", hr, (long long)remaining );
    hr = IXGameSaveImpl3_XGameSaveGetRemainingQuotaAsync( gs, provider, &async );
    hr = IXGameSaveImpl3_XGameSaveGetRemainingQuotaResult( gs, &async, &remaining );
    printf( "GetRemainingQuotaResult: 0x%08lx, %lld bytes\n", hr, (long long)remaining );
    return FAILED( hr );
}

static int cmd_files( const char *scid )
{
    XAsyncBlock async = { 0 };
    char folder[1024];
    SIZE_T size = 0;
    INT64 remaining = -1;
    HRESULT hr;

    hr = IXGameSaveImpl3_XGameSaveFilesGetFolderWithUiAsync( gs, user, scid, &async );
    printf( "FilesGetFolderWithUiAsync: 0x%08lx\n", hr );
    hr = IXThreadingImpl_XAsyncGetResultSize( threading, &async, &size );
    printf( "XAsyncGetResultSize: 0x%08lx, %zu\n", hr, (size_t)size );
    hr = IXGameSaveImpl3_XGameSaveFilesGetFolderWithUiResult( gs, &async, sizeof(folder), folder );
    printf( "FilesGetFolderWithUiResult: 0x%08lx, '%s'\n", hr, SUCCEEDED( hr ) ? folder : "" );
    hr = IXGameSaveImpl3_XGameSaveFilesGetRemainingQuota( gs, user, scid, &remaining );
    printf( "FilesGetRemainingQuota: 0x%08lx, %lld\n", hr, (long long)remaining );
    return FAILED( hr );
}

int main( int argc, char **argv )
{
    HMODULE module;
    InitializeApiImpl_t init;
    QueryApiImpl_t query;
    IXUserImpl *users;
    XAsyncBlock async = { 0 };
    XGameSaveProviderHandle provider = NULL;
    const char *scid, *cmd;
    HRESULT hr;
    int ret;

    if (argc < 3)
    {
        fprintf( stderr, "usage: xgstest <scid> <command> [args]\n" );
        return 2;
    }
    scid = argv[1];
    cmd = argv[2];

    if (!(module = LoadLibraryA( "xgameruntime.dll" )))
    {
        fprintf( stderr, "xgameruntime.dll not found (%lu)\n", GetLastError() );
        return 1;
    }
    init = (InitializeApiImpl_t)GetProcAddress( module, "InitializeApiImpl" );
    query = (QueryApiImpl_t)GetProcAddress( module, "QueryApiImpl" );
    if (!init || !query)
    {
        fprintf( stderr, "missing exports\n" );
        return 1;
    }
    hr = init( 10002, 7822 );
    printf( "InitializeApiImpl: 0x%08lx\n", hr );

    hr = query( &CLSID_XUserImpl, &IID_IXUserImpl, (void **)&users );
    if (FAILED( hr )) { printf( "no XUser: 0x%08lx\n", hr ); return 1; }
    hr = IXUserImpl_XUserAddAsync( users, XUserAddOptions_AddDefaultUserSilently, &async );
    hr = IXUserImpl_XUserAddResult( users, &async, &user );
    printf( "XUserAddResult: 0x%08lx, user %p\n", hr, user );
    if (FAILED( hr )) return 1;

    hr = query( &CLSID_XThreadingImpl, &IID_IXThreadingImpl, (void **)&threading );
    if (FAILED( hr )) { printf( "no XThreading: 0x%08lx\n", hr ); return 1; }
    hr = query( &CLSID_XGameSaveImpl, &IID_IXGameSaveImpl3, (void **)&gs );
    if (FAILED( hr )) { printf( "no XGameSave: 0x%08lx\n", hr ); return 1; }

    if (!strcmp( cmd, "files" )) return cmd_files( scid );

    memset( &async, 0, sizeof(async) );
    hr = IXGameSaveImpl3_XGameSaveInitializeProviderAsync( gs, user, scid, FALSE, &async );
    printf( "InitializeProviderAsync: 0x%08lx\n", hr );
    hr = IXGameSaveImpl3_XGameSaveInitializeProviderResult( gs, &async, &provider );
    printf( "InitializeProviderResult: 0x%08lx, provider %p\n", hr, provider );
    if (FAILED( hr )) return 1;

    if (!strcmp( cmd, "list" ))
    {
        ret = cmd_list( provider );
        if (!ret && argc > 3) ret = cmd_blobs( provider, argv[3] );
    }
    else if (!strcmp( cmd, "blobs" ) && argc > 3) ret = cmd_blobs( provider, argv[3] );
    else if (!strcmp( cmd, "read" ) && argc > 3)
        ret = cmd_read( provider, argv[3], argc > 4 ? (const char **)argv + 4 : NULL, argc - 4 );
    else if (!strcmp( cmd, "readasync" ) && argc > 3) ret = cmd_read_async( provider, argv[3] );
    else if (!strcmp( cmd, "write" ) && argc > 5)
        ret = cmd_write( provider, argv[3], argv[4], argv[5], argc > 6 ? argv[6] : NULL, FALSE );
    else if (!strcmp( cmd, "writeasync" ) && argc > 5)
        ret = cmd_write( provider, argv[3], argv[4], argv[5], argc > 6 ? argv[6] : NULL, TRUE );
    else if (!strcmp( cmd, "delblob" ) && argc > 4) ret = cmd_delblob( provider, argv[3], argv[4] );
    else if (!strcmp( cmd, "delete" ) && argc > 3) ret = cmd_delete( provider, argv[3] );
    else if (!strcmp( cmd, "quota" )) ret = cmd_quota( provider );
    else
    {
        fprintf( stderr, "unknown command %s\n", cmd );
        ret = 2;
    }

    IXGameSaveImpl3_XGameSaveCloseProvider( gs, provider );
    return ret;
}
