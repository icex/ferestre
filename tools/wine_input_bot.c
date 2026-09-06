/*
 * Drive a Wine game's UI from a script, so a reproduction does not need a
 * person clicking through menus.
 *
 * The desktop here is KDE on Wayland, where there is no usable X11 input
 * injection (the game's surface is a Wayland surface and the X root is empty),
 * and no ydotool/xdotool is installed. Running inside the same Wine prefix
 * sidesteps all of that: this joins the game's wineserver session, so SendInput
 * goes into the same input queue the game reads, and window geometry can be
 * asked for directly instead of guessed from a screenshot.
 *
 * Screenshots come from the host side (spectacle), so this only has to deal
 * with input and window geometry.
 *
 *   wine_input_bot.exe windows                 list top-level windows
 *   wine_input_bot.exe wait <substr> <secs>    wait for a window to appear
 *   wine_input_bot.exe resize <substr> <x> <y> <w> <h>
 *                                              move/resize a window, so a test
 *                                              run does not depend on which
 *                                              monitor or resolution is in use
 *   wine_input_bot.exe script <file>           run a script
 *
 * Script commands, one per line, '#' comments:
 *   focus <substr>        bring the matching window to the foreground
 *   move <x> <y>          move the pointer (client coords of the focused window)
 *   click <x> <y>         move and left-click
 *   rclick <x> <y>        move and right-click
 *   key <VK hex>          tap a virtual key
 *   text <string>         type a string
 *   wait <ms>             pause
 *   rect                  print the focused window's rectangle
 */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static HWND g_target;

struct find_ctx
{
    const char *needle;
    HWND found;
};

static BOOL CALLBACK find_proc( HWND hwnd, LPARAM param )
{
    struct find_ctx *ctx = (struct find_ctx *)param;
    char title[512];

    if (!IsWindowVisible( hwnd )) return TRUE;
    if (!GetWindowTextA( hwnd, title, sizeof(title) )) return TRUE;
    if (!title[0]) return TRUE;
    if (strstr( title, ctx->needle ))
    {
        ctx->found = hwnd;
        return FALSE;
    }
    return TRUE;
}

static BOOL CALLBACK list_proc( HWND hwnd, LPARAM param )
{
    char title[512], cls[256];
    RECT r;

    if (!IsWindowVisible( hwnd )) return TRUE;
    GetWindowTextA( hwnd, title, sizeof(title) );
    GetClassNameA( hwnd, cls, sizeof(cls) );
    GetWindowRect( hwnd, &r );
    if (!title[0] && !cls[0]) return TRUE;
    printf( "hwnd %p  rect %ld,%ld %ldx%ld  class '%s'  title '%s'\n",
            hwnd, r.left, r.top, r.right - r.left, r.bottom - r.top, cls, title );
    return TRUE;
}

static HWND find_window( const char *needle )
{
    struct find_ctx ctx = { needle, NULL };

    EnumWindows( find_proc, (LPARAM)&ctx );
    return ctx.found;
}

/* SendInput's absolute coordinates are normalised over the virtual desktop. */
static void move_to( int x, int y )
{
    INPUT in = { 0 };
    int w = GetSystemMetrics( SM_CXVIRTUALSCREEN ), h = GetSystemMetrics( SM_CYVIRTUALSCREEN );
    int ox = GetSystemMetrics( SM_XVIRTUALSCREEN ), oy = GetSystemMetrics( SM_YVIRTUALSCREEN );

    if (w <= 0) w = 1;
    if (h <= 0) h = 1;
    in.type = INPUT_MOUSE;
    in.mi.dx = (LONG)(((LONGLONG)(x - ox) * 65535) / w);
    in.mi.dy = (LONG)(((LONGLONG)(y - oy) * 65535) / h);
    in.mi.dwFlags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
    SendInput( 1, &in, sizeof(in) );
}

static void button( DWORD down, DWORD up )
{
    INPUT in = { 0 };

    in.type = INPUT_MOUSE;
    in.mi.dwFlags = down;
    SendInput( 1, &in, sizeof(in) );
    Sleep( 40 );
    in.mi.dwFlags = up;
    SendInput( 1, &in, sizeof(in) );
}

static void tap_key( WORD vk )
{
    INPUT in = { 0 };

    in.type = INPUT_KEYBOARD;
    in.ki.wVk = vk;
    SendInput( 1, &in, sizeof(in) );
    Sleep( 40 );
    in.ki.dwFlags = KEYEVENTF_KEYUP;
    SendInput( 1, &in, sizeof(in) );
}

/* Client coordinates of the focused window, so a script does not depend on
 * where the window happens to sit on the desktop. */
static void client_to_screen_xy( int *x, int *y )
{
    POINT p = { *x, *y };

    if (g_target && ClientToScreen( g_target, &p ))
    {
        *x = p.x;
        *y = p.y;
    }
}

static void focus( const char *needle )
{
    HWND hwnd = find_window( needle );

    if (!hwnd)
    {
        printf( "  focus '%s': not found\n", needle );
        return;
    }
    g_target = hwnd;
    ShowWindow( hwnd, SW_RESTORE );
    SetForegroundWindow( hwnd );
    SetActiveWindow( hwnd );
    Sleep( 200 );
    printf( "  focus '%s' -> %p\n", needle, hwnd );
}

static void run_script( const char *path )
{
    char line[1024];
    FILE *f = fopen( path, "r" );

    if (!f) { printf( "cannot open %s\n", path ); return; }

    while (fgets( line, sizeof(line), f ))
    {
        char *nl = strpbrk( line, "\r\n" );
        int x, y;
        unsigned vk;

        if (nl) *nl = 0;
        if (!line[0] || line[0] == '#') continue;
        printf( "> %s\n", line );
        fflush( stdout );

        if (!strncmp( line, "focus ", 6 )) focus( line + 6 );
        else if (sscanf( line, "click %d %d", &x, &y ) == 2)
        {
            client_to_screen_xy( &x, &y );
            move_to( x, y );
            Sleep( 120 );
            button( MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP );
        }
        else if (sscanf( line, "rclick %d %d", &x, &y ) == 2)
        {
            client_to_screen_xy( &x, &y );
            move_to( x, y );
            Sleep( 120 );
            button( MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP );
        }
        else if (sscanf( line, "move %d %d", &x, &y ) == 2)
        {
            client_to_screen_xy( &x, &y );
            move_to( x, y );
        }
        else if (sscanf( line, "key %x", &vk ) == 1) tap_key( (WORD)vk );
        else if (!strncmp( line, "text ", 5 ))
        {
            const char *s = line + 5;
            for (; *s; s++)
            {
                SHORT v = VkKeyScanA( *s );
                if (v == -1) continue;
                tap_key( (WORD)(v & 0xff) );
            }
        }
        else if (sscanf( line, "wait %d", &x ) == 1) Sleep( x );
        else if (!strcmp( line, "rect" ))
        {
            RECT r = { 0 };
            if (g_target)
            {
                GetClientRect( g_target, &r );
                printf( "  client %ldx%ld\n", r.right, r.bottom );
                GetWindowRect( g_target, &r );
                printf( "  window %ld,%ld %ldx%ld\n", r.left, r.top, r.right - r.left, r.bottom - r.top );
            }
        }
        else printf( "  ? unrecognised\n" );
        fflush( stdout );
    }
    fclose( f );
}

int main( int argc, char **argv )
{
    setvbuf( stdout, NULL, _IONBF, 0 );

    if (argc > 1 && !strcmp( argv[1], "windows" ))
    {
        EnumWindows( list_proc, 0 );
        return 0;
    }
    if (argc > 3 && !strcmp( argv[1], "wait" ))
    {
        int secs = atoi( argv[3] ), i;

        for (i = 0; i < secs * 4; i++)
        {
            HWND hwnd = find_window( argv[2] );

            if (hwnd) { printf( "found %p\n", hwnd ); return 0; }
            Sleep( 250 );
        }
        printf( "timed out waiting for '%s'\n", argv[2] );
        return 1;
    }
    if (argc > 6 && !strcmp( argv[1], "resize" ))
    {
        HWND hwnd = find_window( argv[2] );

        if (!hwnd) { printf( "resize: '%s' not found\n", argv[2] ); return 1; }
        /* SetWindowPos will not resize a maximized window, and the game comes
         * up maximized on whatever display is attached, so drop it out of that
         * state first. ShowWindow on another process's window is posted, not
         * called, so it is safe even when that process is busy. */
        if (IsZoomed( hwnd ))
        {
            ShowWindow( hwnd, SW_RESTORE );
            Sleep( 600 );
        }
        /* SWP_NOACTIVATE: never touch the foreground, so this stays usable
         * against a window whose owner is not pumping messages. */
        SetWindowPos( hwnd, NULL, atoi( argv[3] ), atoi( argv[4] ), atoi( argv[5] ), atoi( argv[6] ),
                      SWP_NOACTIVATE | SWP_NOZORDER | SWP_ASYNCWINDOWPOS );
        Sleep( 400 );
        {
            RECT r;
            GetWindowRect( hwnd, &r );
            printf( "resized to %ld,%ld %ldx%ld\n", r.left, r.top, r.right - r.left, r.bottom - r.top );
        }
        return 0;
    }
    if (argc > 2 && !strcmp( argv[1], "script" ))
    {
        run_script( argv[2] );
        return 0;
    }
    printf( "usage: %s windows | wait <substr> <secs> | script <file>\n", argv[0] );
    return 2;
}
