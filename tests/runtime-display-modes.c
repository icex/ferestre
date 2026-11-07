/* Exercise the runtime's public display API, without creating a game window. */
#include <windows.h>
#include <stdio.h>

int WINAPI WinMain(HINSTANCE instance, HINSTANCE previous, LPSTR command, int show)
{
    static const SIZE required[] = {{3840, 2160}, {3440, 1440}, {3840, 1080}, {5120, 1440}};
    BOOL found[4] = {FALSE, FALSE, FALSE, FALSE}, native = FALSE;
    DEVMODEW current, mode;
    char path[4096];
    FILE *output;
    DWORD index;
    unsigned int i, missing = 0;

    if (!GetEnvironmentVariableA("FERESTRE_MODE_PROBE_OUTPUT", path, sizeof(path))) return 2;
    if (!(output = fopen(path, "w"))) return 3;
    ZeroMemory(&current, sizeof(current));
    current.dmSize = sizeof(current);
    if (!EnumDisplaySettingsW(NULL, ENUM_CURRENT_SETTINGS, &current)) return 4;
    fprintf(output, "desktop=%lux%lu@%lu\n", current.dmPelsWidth, current.dmPelsHeight,
            current.dmDisplayFrequency);
    if (current.dmPelsWidth < 7680 || current.dmPelsHeight < 2160) return 5;
    for (index = 0; index < 8192; ++index)
    {
        ZeroMemory(&mode, sizeof(mode));
        mode.dmSize = sizeof(mode);
        if (!EnumDisplaySettingsW(NULL, index, &mode)) break;
        if (mode.dmBitsPerPel != 32 || mode.dmDisplayFrequency != current.dmDisplayFrequency) continue;
        if (mode.dmPelsWidth == current.dmPelsWidth && mode.dmPelsHeight == current.dmPelsHeight)
            native = TRUE;
        for (i = 0; i < 4; ++i)
            if (mode.dmPelsWidth == required[i].cx && mode.dmPelsHeight == required[i].cy)
                found[i] = TRUE;
    }
    for (i = 0; i < 4; ++i)
    {
        fprintf(output, "%ldx%ld@%lu=%s\n", required[i].cx, required[i].cy,
                current.dmDisplayFrequency, found[i] ? "present" : "MISSING");
        if (!found[i]) ++missing;
    }
    fprintf(output, "native_preserved=%d missing=%u\n", native, missing);
    fclose(output);
    return missing || !native;
}
