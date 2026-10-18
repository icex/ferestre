/* Exercise the actual gameinput.dll against a deterministic XInput backend. */
#include <windows.h>
#include <xinput.h>
#include <stdio.h>
#include <string.h>
#define COBJMACROS
#include "initguid.h"
#include "gameinput.h"

static unsigned int failures, count;
static IGameInputDevice_v2 *devices[4];
#define OK(c, ...) do { printf("%s - ", (c) ? "ok  " : "FAIL"); printf(__VA_ARGS__); printf("\n"); if (!(c)) failures++; } while (0)

/* The pad a reading came from, without keeping a reference to it. */
static IGameInputDevice_v2 *reading_device(IGameInputReading_v2 *reading)
{
    IGameInputDevice_v2 *device = NULL;

    IGameInputReading_v2_GetDevice(reading, &device);
    if (device) IGameInputDevice_v2_Release(device);
    return device;
}

static GameInputGamepadButtons reading_buttons(IGameInputReading_v2 *reading)
{
    GameInputGamepadState pad = {0};

    IGameInputReading_v2_GetGamepadState(reading, &pad);
    return pad.buttons;
}

static void WINAPI callback(GameInputCallbackToken token, void *context, IGameInputDevice_v2 *device,
                           uint64_t timestamp, GameInputDeviceStatus current, GameInputDeviceStatus previous)
{
    if (count < 4) devices[count] = device;
    count++;
}

/* Run this thread's messages for a while. */
static void pump(DWORD ms)
{
    DWORD start = GetTickCount();
    MSG msg;

    while (GetTickCount() - start < ms)
    {
        while (PeekMessageA(&msg, NULL, 0, 0, PM_REMOVE)) DispatchMessageA(&msg);
        Sleep(5);
    }
}

/* Where the process's raw mouse input goes, if anywhere. Raw input has one
 * registration per process for each kind of device. */
static BOOL mouse_registration(HWND *target)
{
    RAWINPUTDEVICE list[32];
    UINT size = sizeof(list) / sizeof(list[0]), found, i;

    *target = NULL;
    if ((found = GetRegisteredRawInputDevices(list, &size, sizeof(*list))) == (UINT)-1) return FALSE;
    for (i = 0; i < found; i++)
    {
        if (list[i].usUsagePage != 0x01 || list[i].usUsage != 0x02) continue; /* generic desktop mouse */
        *target = list[i].hwndTarget;
        return TRUE;
    }
    return FALSE;
}

static BOOL set_mouse_registration(HWND target, DWORD flags)
{
    RAWINPUTDEVICE mouse = {0x01, 0x02, flags, target};
    return RegisterRawInputDevices(&mouse, 1, sizeof(mouse));
}

/* GameInput reads mouse motion through raw input, which it shares with the
 * title and with dinput: a title's own registration must survive a GameInput
 * mouse reading, and GameInput must pick the mouse back up once whoever held it
 * lets go, as dinput does when it unacquires. */
static void test_mouse_registration(IGameInput_v2 *input)
{
    IGameInputReading_v2 *reading = NULL;
    GameInputMouseState_v2 s0, s1;
    HWND title, target, foreground;
    char flag[8] = "";
    BOOL pointer;
    INPUT in;
    HRESULT hr;

    /* Moving the pointer is only safe on a private display (Xvfb), since on a
     * real one it moves the user's pointer, and Wine's null driver never moves
     * it at all. So those checks run only when asked for. */
    pointer = GetEnvironmentVariableA("GAMEINPUT_TEST_POINTER", flag, sizeof(flag)) && flag[0] == '1';
    title = CreateWindowExA(0, "STATIC", "title", 0, 0, 0, 0, 0, HWND_MESSAGE, NULL, NULL, NULL);
    OK(title && set_mouse_registration(title, 0), "the title registers its own raw mouse");
    hr = IGameInput_v2_GetCurrentReading(input, GameInputKindMouse, NULL, &reading);
    OK(hr == S_OK && reading, "mouse reading");
    if (!reading) goto done;

    IGameInputReading_v2_GetMouseState(reading, &s0);
    Sleep(200); /* long enough for a registration from GameInput's own thread to land */
    IGameInputReading_v2_GetMouseState(reading, &s0);
    OK(mouse_registration(&target) && target == title, "a mouse reading leaves the title's raw mouse registration alone");

    if (pointer)
    {
        SetCursorPos(100, 100);
        IGameInputReading_v2_GetMouseState(reading, &s0);
        SetCursorPos(130, 120);
        IGameInputReading_v2_GetMouseState(reading, &s1);
        OK(s1.positionX - s0.positionX == 30 && s1.positionY - s0.positionY == 20,
           "while the title holds raw input, motion comes from the cursor (%lld,%lld)",
           (long long)(s1.positionX - s0.positionX), (long long)(s1.positionY - s0.positionY));
    }
    else printf("ok   - # SKIP cursor fallback (set GAMEINPUT_TEST_POINTER=1 on a private display)\n");

    set_mouse_registration(NULL, RIDEV_REMOVE);
    OK(!mouse_registration(&target), "nothing holds the raw mouse after RIDEV_REMOVE");
    IGameInputReading_v2_GetMouseState(reading, &s0);
    OK(mouse_registration(&target) && target && target != title, "GameInput takes the raw mouse once it is free again");

    if (pointer)
    {
        SetCursorPos(150, 150);
        IGameInputReading_v2_GetMouseState(reading, &s0);
        SetCursorPos(170, 175);
        IGameInputReading_v2_GetMouseState(reading, &s1);
        OK(s1.positionX == s0.positionX && s1.positionY == s0.positionY,
           "with raw input its own, cursor motion is not counted a second time");

        /* And raw motion reaches it again, as after dinput unacquires. Wine
         * delivers raw input only while the process owns the foreground
         * window, and the display may scale the motion, so only its direction
         * is checked. */
        foreground = CreateWindowExA(0, "STATIC", "gameinput_gamepad_tests", WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                                     0, 0, 320, 240, NULL, NULL, NULL, NULL);
        SetForegroundWindow(foreground);
        pump(300);
        IGameInputReading_v2_GetMouseState(reading, &s0);
        memset(&in, 0, sizeof(in));
        in.type = INPUT_MOUSE;
        in.mi.dx = 30;
        in.mi.dy = 20;
        in.mi.dwFlags = MOUSEEVENTF_MOVE;
        SendInput(1, &in, sizeof(in));
        pump(300);
        IGameInputReading_v2_GetMouseState(reading, &s1);
        OK(s1.positionX > s0.positionX && s1.positionY > s0.positionY,
           "raw motion reaches GameInput again once it holds the mouse (%lld,%lld)",
           (long long)(s1.positionX - s0.positionX), (long long)(s1.positionY - s0.positionY));
        DestroyWindow(foreground);
    }

    set_mouse_registration(title, 0);
    IGameInputReading_v2_GetMouseState(reading, &s0);
    OK(mouse_registration(&target) && target == title, "a title registering again keeps its registration");
    set_mouse_registration(NULL, RIDEV_REMOVE);

done:
    if (reading) IGameInputReading_v2_Release(reading);
    if (title) DestroyWindow(title);
}

int main(void)
{
    HRESULT (WINAPI *create)(IGameInput_v0 **);
    void (WINAPI *set_state)(DWORD, const XINPUT_STATE *, BOOL);
    void (WINAPI *get_vibration)(DWORD, XINPUT_VIBRATION *);
    HMODULE fixture = LoadLibraryA("xinput1_3.dll"), module = LoadLibraryA("gameinput.dll");
    IGameInput_v0 *v0 = NULL;
    IGameInput_v2 *input = NULL;
    IGameInputReading_v2 *reading = NULL, *next = NULL, *first = NULL, *second = NULL, *any = NULL;
    IGameInputDevice_v2 *found = NULL;
    IGameInputDevice_v0 *device_v0 = NULL;
    IUnknown *unknown = NULL;
    const GameInputDeviceInfo_v2 *info;
    const GameInputDeviceInfo_v0 *info_v0;
    GameInputGamepadState pad, original;
    GameInputCallbackToken token;
    GameInputRumbleParams rumble = {1.0f, 0.5f, 0, 0};
    XINPUT_VIBRATION vibration;
    XINPUT_STATE state = {7, {XINPUT_GAMEPAD_A | XINPUT_GAMEPAD_START | XINPUT_GAMEPAD_DPAD_LEFT,
                             255, 128, -32768, 32767, 0, -16384}};
    XINPUT_STATE other;
    uint64_t stamp;
    HRESULT hr;

    create = (void *)GetProcAddress(module, "GameInputCreate");
    set_state = (void *)GetProcAddress(fixture, "FixtureSetState");
    get_vibration = (void *)GetProcAddress(fixture, "FixtureGetVibration");
    OK(create && set_state && get_vibration, "real GameInput and test-only XInput backend load");
    if (!create || !set_state || !get_vibration) goto done;
    set_state(0, &state, TRUE);
    set_state(3, &state, TRUE);
    hr = create(&v0);
    OK(hr == S_OK && v0, "GameInputCreate");
    if (!v0) goto done;
    IGameInput_v0_QueryInterface(v0, &IID_IGameInput_v2, (void **)&input);
    OK(input != NULL, "IGameInput_v2 available");
    if (!input) goto done;
    hr = IGameInput_v2_RegisterDeviceCallback(input, NULL, GameInputKindGamepad, 0x200001,
                                             GameInputBlockingEnumeration, NULL, callback, &token);
    OK(hr == S_OK && count == 2, "enumerates exactly the two connected pads, including slot 3");
    if (count != 2) goto done;
    IGameInputDevice_v2_GetDeviceInfo(devices[0], &info);
    OK(info->supportedInput == GameInputKindGamepad && info->gamepadInfo, "gamepad metadata present");
    OK(info->vendorId == 0x045e && info->productId == 0x028e, "v2 info names the Xbox 360 pad XInput presents");
    hr = IGameInputDevice_v2_QueryInterface(devices[0], &IID_IGameInputDevice_v0, (void **)&device_v0);
    OK(hr == S_OK && device_v0, "pad from the v2 callback answers IGameInputDevice_v0");
    if (device_v0)
    {
        info_v0 = IGameInputDevice_v0_GetDeviceInfo(device_v0);
        OK(info_v0 && info_v0->vendorId == info->vendorId && info_v0->productId == info->productId,
           "v0 and v2 views of one pad report the same vendor and product");
        IGameInputDevice_v0_QueryInterface(device_v0, &IID_IUnknown, (void **)&unknown);
        OK(unknown == (IUnknown *)devices[0], "v0 view shares the pad's IUnknown identity");
        if (unknown) unknown->lpVtbl->Release(unknown);
        IGameInputDevice_v0_Release(device_v0);
    }
    IGameInput_v2_FindDeviceFromId(input, &info->deviceId, &found);
    OK(found == devices[0], "FindDeviceFromId preserves gamepad identity");
    if (found) IGameInputDevice_v2_Release(found);
    hr = IGameInput_v2_GetCurrentReading(input, GameInputKindGamepad, devices[0], &reading);
    OK(hr == S_OK && reading, "connected pad supplies a reading");
    if (!reading) goto done;
    OK(IGameInputReading_v2_GetGamepadState(reading, &pad), "reading supplies gamepad state");
    OK(pad.buttons == (GameInputGamepadA | GameInputGamepadMenu | GameInputGamepadDPadLeft), "button mapping");
    OK(pad.leftTrigger == 1.0f && pad.rightTrigger > 0.501f && pad.rightTrigger < 0.503f, "trigger normalization");
    OK(pad.leftThumbstickX == -1.0f && pad.leftThumbstickY == 1.0f && pad.rightThumbstickY < -0.49f,
       "signed stick endpoints and direction");
    original = pad;
    hr = IGameInput_v2_GetNextReading(input, reading, GameInputKindGamepad, devices[0], &next);
    OK(hr == GAMEINPUT_E_READING_NOT_FOUND && !next, "unchanged packet terminates a drain loop");
    state.dwPacketNumber++;
    state.Gamepad.wButtons = XINPUT_GAMEPAD_B;
    set_state(0, &state, TRUE);
    hr = IGameInput_v2_GetNextReading(input, reading, GameInputKindGamepad, devices[0], &next);
    OK(hr == S_OK && next, "changed packet supplies next reading");
    if (next)
    {
        IGameInputReading_v2_GetGamepadState(next, &pad);
        OK(pad.buttons == GameInputGamepadB, "next reading contains the new input");
        IGameInputReading_v2_Release(next);
    }
    IGameInputReading_v2_GetGamepadState(reading, &pad);
    OK(!memcmp(&pad, &original, sizeof(pad)), "retained reading stays immutable");
    IGameInputDevice_v2_SetRumbleState(devices[0], &rumble);
    get_vibration(0, &vibration);
    OK(vibration.wLeftMotorSpeed == 65535 && vibration.wRightMotorSpeed == 32767, "rumble reaches XInput");

    /* An idle pad's timestamp is when its packet arrived, not when it was polled. */
    hr = IGameInput_v2_GetCurrentReading(input, GameInputKindGamepad, devices[0], &first);
    Sleep(20);
    if (hr == S_OK) hr = IGameInput_v2_GetCurrentReading(input, GameInputKindGamepad, devices[0], &second);
    OK(hr == S_OK && first && second, "idle pad supplies repeated readings");
    if (first && second)
    {
        stamp = IGameInputReading_v2_GetTimestamp(first);
        OK(IGameInputReading_v2_GetTimestamp(second) == stamp, "idle pad's timestamp does not advance between polls");
        OK(stamp <= IGameInput_v2_GetCurrentTimestamp(input), "pad timestamps share GetCurrentTimestamp's clock");
        IGameInputReading_v2_Release(second);
        second = NULL;
        state.dwPacketNumber++;
        set_state(0, &state, TRUE);
        IGameInput_v2_GetCurrentReading(input, GameInputKindGamepad, devices[0], &second);
        OK(second && IGameInputReading_v2_GetTimestamp(second) > stamp, "a new packet gets a newer timestamp");
    }
    if (first) IGameInputReading_v2_Release(first);
    if (second) IGameInputReading_v2_Release(second);
    first = second = NULL;

    /* With no device, readings come from whichever pad was used last, as in
     * GameInput's single input stream: not always from the lowest slot. */
    hr = IGameInput_v2_GetCurrentReading(input, GameInputKindGamepad, NULL, &first);
    OK(hr == S_OK && first && reading_device(first) == devices[0], "any-pad reading starts at the pad in use");
    other = state;
    other.dwPacketNumber = 100;
    other.Gamepad.wButtons = XINPUT_GAMEPAD_X;
    set_state(3, &other, TRUE);
    hr = IGameInput_v2_GetCurrentReading(input, GameInputKindGamepad, NULL, &any);
    OK(hr == S_OK && any && reading_device(any) == devices[1] && reading_buttons(any) == GameInputGamepadX,
       "any-pad reading follows input on the pad in slot 3 after slot 0");
    if (any) IGameInputReading_v2_Release(any);
    any = NULL;
    if (first)
    {
        hr = IGameInput_v2_GetNextReading(input, first, GameInputKindGamepad, NULL, &second);
        OK(hr == S_OK && second && reading_device(second) == devices[1] && reading_buttons(second) == GameInputGamepadX,
           "next any-pad reading after slot 0's is the newer input from slot 3");
    }
    if (second)
    {
        hr = IGameInput_v2_GetNextReading(input, second, GameInputKindGamepad, NULL, &any);
        OK(hr == GAMEINPUT_E_READING_NOT_FOUND && !any, "no newer input on any pad ends the drain loop");
        if (any) IGameInputReading_v2_Release(any);
        any = NULL;
        hr = IGameInput_v2_GetNextReading(input, second, GameInputKindGamepad, devices[0], &any);
        OK(hr == GAMEINPUT_E_READING_NOT_FOUND && !any, "a named pad with no input since the reference has no next reading");
        if (any) IGameInputReading_v2_Release(any);
        any = NULL;
        state.dwPacketNumber++;
        state.Gamepad.wButtons = XINPUT_GAMEPAD_Y;
        set_state(0, &state, TRUE);
        hr = IGameInput_v2_GetNextReading(input, second, GameInputKindGamepad, NULL, &any);
        OK(hr == S_OK && any && reading_device(any) == devices[0] && reading_buttons(any) == GameInputGamepadY,
           "slot 0 used again supplies the next any-pad reading");
        if (any) IGameInputReading_v2_Release(any);
        any = NULL;
    }
    hr = IGameInput_v2_GetCurrentReading(input, GameInputKindGamepad, NULL, &any);
    OK(hr == S_OK && any && reading_device(any) == devices[0], "any-pad reading returns to slot 0 once it is used again");
    if (any) IGameInputReading_v2_Release(any);
    if (first) IGameInputReading_v2_Release(first);
    if (second) IGameInputReading_v2_Release(second);
    any = first = second = NULL;

    set_state(0, &state, FALSE);
    OK(IGameInputDevice_v2_GetDeviceStatus(devices[0]) == GameInputDeviceNoStatus, "disconnected pad reports disconnected");
    next = NULL;
    hr = IGameInput_v2_GetCurrentReading(input, GameInputKindGamepad, devices[0], &next);
    OK(hr == GAMEINPUT_E_READING_NOT_FOUND && !next, "disconnected pad supplies no reading");
    IGameInput_v2_UnregisterCallback(input, token);

    test_mouse_registration(input);

done:
    if (reading) IGameInputReading_v2_Release(reading);
    if (input) IGameInput_v2_Release(input);
    if (v0) IGameInput_v0_Release(v0);
    printf("%s (%u failures)\n", failures ? "FAILED" : "PASSED", failures);
    return failures != 0;
}
