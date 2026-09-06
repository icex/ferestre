/*
 * Exercise the XPackage implementation in xgameruntime.dll without a game.
 *
 * Loads the DLL, initialises it, gets the XPackage interface via QueryApiImpl,
 * and calls the functions a GDK title uses at startup -- identity, is-packaged,
 * locale, package enumeration, and a mount + mount-path round-trip. Run it from
 * a directory that has a MicrosoftGame.config so the identity resolves.
 *
 * Build (inside the Proton SDK container):
 *   x86_64-w64-mingw32-gcc -o xpkgtest.exe xpkgtest.c -D__WINESRC__ -DCOBJMACROS \
 *       -I<obj>/dlls/xgameruntime -I<src>/dlls/xgameruntime
 */

#define COBJMACROS
#include <windows.h>
#include <initguid.h>
#include <stdio.h>
#include <string.h>

#include "xpackage.h"
#include "xasyncprovider.h"

typedef HRESULT (WINAPI *InitializeApiImpl_t)( ULONG, ULONG );
typedef HRESULT (WINAPI *QueryApiImpl_t)( REFCLSID, REFIID, void ** );

static IXThreadingImpl *threading;

static BOOLEAN __stdcall on_package( void *ctx, const XPackageDetails *d )
{
    printf( "  package '%s'  display='%s'  title='%s'  kind=%d  version=%u.%u.%u.%u  index=%u/%u\n",
            d->packageIdentifier, d->displayName, d->titleID ? d->titleID : "", d->kind,
            d->version.major, d->version.minor, d->version.build, d->version.revision,
            d->index, d->count );
    (*(int *)ctx)++;
    return TRUE;
}

int main( void )
{
    HMODULE mod;
    InitializeApiImpl_t init;
    QueryApiImpl_t query;
    IXPackageImpl3 *pkg;
    char buf[512];
    HRESULT hr;
    int count = 0;

    if (!(mod = LoadLibraryA( "xgameruntime.dll" )))
    {
        fprintf( stderr, "load failed %lu\n", GetLastError() );
        return 1;
    }
    init = (InitializeApiImpl_t)GetProcAddress( mod, "InitializeApiImpl" );
    query = (QueryApiImpl_t)GetProcAddress( mod, "QueryApiImpl" );
    init( 10002, 7822 );

    hr = query( &CLSID_XThreadingImpl, &IID_IXThreadingImpl, (void **)&threading );
    hr = query( &CLSID_XPackageImpl, &IID_IXPackageImpl3, (void **)&pkg );
    printf( "QueryApiImpl(XPackage): 0x%08lx\n", hr );
    if (FAILED( hr )) return 1;

    printf( "IsPackagedProcess: %d\n", IXPackageImpl3_XPackageIsPackagedProcess( pkg ) );

    hr = IXPackageImpl3_XPackageGetCurrentProcessPackageIdentifier( pkg, sizeof(buf), buf );
    printf( "GetCurrentProcessPackageIdentifier: 0x%08lx  '%s'\n", hr, SUCCEEDED( hr ) ? buf : "" );

    hr = IXPackageImpl3_XPackageGetUserLocale( pkg, sizeof(buf), buf );
    printf( "GetUserLocale: 0x%08lx  '%s'\n", hr, SUCCEEDED( hr ) ? buf : "" );

    hr = IXPackageImpl3_XPackageEnumeratePackages( pkg, XPackageKind_Game,
            XPackageEnumerationScope_ThisOnly, &count, on_package );
    printf( "EnumeratePackages(Game): 0x%08lx, %d package(s)\n", hr, count );

    count = 0;
    hr = IXPackageImpl3_XPackageEnumeratePackages( pkg, XPackageKind_Content,
            XPackageEnumerationScope_ThisOnly, &count, on_package );
    printf( "EnumeratePackages(Content): 0x%08lx, %d package(s) (expect 0)\n", hr, count );

    /* mount + mount path round-trip */
    {
        XAsyncBlock async = { 0 };
        XPackageMountHandle mount = NULL;
        SIZE_T size = 0;

        hr = IXPackageImpl3_XPackageMountWithUiAsync( pkg, buf, &async );
        hr = IXPackageImpl3_XPackageMountWithUiResult( pkg, &async, &mount );
        printf( "MountWithUi: 0x%08lx, handle %p\n", hr, mount );
        if (SUCCEEDED( hr ) && mount)
        {
            hr = IXPackageImpl3_XPackageGetMountPathSize( pkg, mount, &size );
            printf( "GetMountPathSize: 0x%08lx, %zu\n", hr, (size_t)size );
            hr = IXPackageImpl3_XPackageGetMountPath( pkg, mount, sizeof(buf), buf );
            printf( "GetMountPath: 0x%08lx  '%s'\n", hr, SUCCEEDED( hr ) ? buf : "" );
            IXPackageImpl3_XPackageCloseMountHandle( pkg, mount );
        }
    }

    /* chunk availability + installation progress */
    {
        XPackageChunkAvailability avail = 99;
        hr = IXPackageImpl3_XPackageFindChunkAvailability( pkg, buf, 0, NULL, &avail );
        printf( "FindChunkAvailability: 0x%08lx, availability=%d (0=Ready)\n", hr, avail );
    }
    {
        XPackageInstallationMonitorHandle mon = NULL;
        XPackageInstallationProgress prog = { 0 };
        hr = IXPackageImpl3_XPackageCreateInstallationMonitor( pkg, buf, 0, NULL, 0, NULL, &mon );
        printf( "CreateInstallationMonitor: 0x%08lx, handle %p\n", hr, mon );
        if (mon)
        {
            IXPackageImpl3_XPackageGetInstallationProgress( pkg, mon, &prog );
            printf( "  progress: installed=%llu/%llu launchable=%d completed=%d\n",
                    (unsigned long long)prog.installedBytes, (unsigned long long)prog.totalBytes,
                    prog.launchable, prog.completed );
            IXPackageImpl3_XPackageCloseInstallationMonitorHandle( pkg, mon );
        }
    }
    return 0;
}
