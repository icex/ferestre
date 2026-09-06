/*
 * Decode a video file through Media Foundation, the path an Unreal Engine
 * title uses for its .mp4 movies on Windows/WinGDK.
 *
 * Under Proton, Media Foundation is winegstreamer, so this answers the one
 * question the game itself can only answer by reaching a loading screen: does
 * the file decode to real frames, or does the media converter substitute the
 * MEDIACONV_BLANK_VIDEO_FILE placeholder?
 *
 * A placeholder is easy to recognise without looking at a screen: the frames
 * come out a different size from the file's own resolution, and every frame is
 * near-identical, so the running checksum of the first rows barely moves.
 *
 *   mftest.exe <path to video>
 *
 * Build (mingw, inside the Proton SDK container):
 *   x86_64-w64-mingw32-gcc -o mftest.exe mftest.c -lmfplat -lmfreadwrite -lmfuuid -lole32
 */

#define COBJMACROS
#define INITGUID
#include <windows.h>
#include <mfapi.h>
#include <mfidl.h>
#include <mfreadwrite.h>
#include <mferror.h>
#include <stdio.h>

static const char *fourcc_name( DWORD f )
{
    static char buf[8];
    if (IsEqualGUID( &MFVideoFormat_NV12, &(GUID){f} )) return "NV12";
    memcpy( buf, &f, 4 );
    buf[4] = 0;
    return buf;
}

int wmain( int argc, WCHAR **argv )
{
    IMFSourceReader *reader = NULL;
    IMFMediaType *native = NULL, *want = NULL, *actual = NULL;
    IMFSample *sample;
    IMFMediaBuffer *buffer;
    UINT32 w = 0, h = 0, num = 0, den = 0;
    UINT64 frame_size = 0, frame_rate = 0;
    DWORD flags, samples = 0, total_bytes = 0;
    LONGLONG ts;
    GUID subtype;
    unsigned int checksum = 0, prev_checksum = 0, changed = 0;
    HRESULT hr;

    if (argc < 2)
    {
        fprintf( stderr, "usage: mftest <video file>\n" );
        return 2;
    }

    if (FAILED( hr = CoInitializeEx( NULL, COINIT_APARTMENTTHREADED ) ))
    {
        printf( "CoInitializeEx: 0x%08lx\n", hr );
        return 1;
    }
    if (FAILED( hr = MFStartup( MF_VERSION, MFSTARTUP_FULL ) ))
    {
        printf( "MFStartup: 0x%08lx\n", hr );
        return 1;
    }

    hr = MFCreateSourceReaderFromURL( argv[1], NULL, &reader );
    printf( "MFCreateSourceReaderFromURL: 0x%08lx\n", hr );
    if (FAILED( hr )) return 1;

    /* what the file itself says it holds */
    hr = IMFSourceReader_GetNativeMediaType( reader, MF_SOURCE_READER_FIRST_VIDEO_STREAM, 0, &native );
    printf( "GetNativeMediaType: 0x%08lx\n", hr );
    if (SUCCEEDED( hr ))
    {
        IMFMediaType_GetGUID( native, &MF_MT_SUBTYPE, &subtype );
        IMFMediaType_GetUINT64( native, &MF_MT_FRAME_SIZE, &frame_size );
        IMFMediaType_GetUINT64( native, &MF_MT_FRAME_RATE, &frame_rate );
        w = frame_size >> 32;
        h = (UINT32)frame_size;
        num = frame_rate >> 32;
        den = (UINT32)frame_rate;
        printf( "  native: %ux%u @ %.2f fps, subtype %s\n", w, h,
                den ? (double)num / den : 0.0, fourcc_name( subtype.Data1 ) );
    }

    /* ask for decoded frames: this is what forces a real decoder to be plugged */
    if (SUCCEEDED( MFCreateMediaType( &want ) ))
    {
        IMFMediaType_SetGUID( want, &MF_MT_MAJOR_TYPE, &MFMediaType_Video );
        IMFMediaType_SetGUID( want, &MF_MT_SUBTYPE, &MFVideoFormat_NV12 );
        hr = IMFSourceReader_SetCurrentMediaType( reader, MF_SOURCE_READER_FIRST_VIDEO_STREAM, NULL, want );
        printf( "SetCurrentMediaType(NV12): 0x%08lx%s\n", hr,
                FAILED( hr ) ? "  <-- no decoder could be plugged" : "" );
        if (FAILED( hr )) return 1;
    }

    if (SUCCEEDED( IMFSourceReader_GetCurrentMediaType( reader, MF_SOURCE_READER_FIRST_VIDEO_STREAM, &actual ) ))
    {
        UINT64 out_size = 0;
        IMFMediaType_GetUINT64( actual, &MF_MT_FRAME_SIZE, &out_size );
        printf( "  decoding to %ux%u\n", (UINT32)(out_size >> 32), (UINT32)out_size );
        if (w && ((UINT32)(out_size >> 32) != w || (UINT32)out_size != h))
            printf( "  !! output size differs from the file: a substitute stream is being played\n" );
    }

    while (samples < 30)
    {
        hr = IMFSourceReader_ReadSample( reader, MF_SOURCE_READER_FIRST_VIDEO_STREAM, 0, NULL, &flags, &ts, &sample );
        if (FAILED( hr ))
        {
            printf( "ReadSample: 0x%08lx\n", hr );
            break;
        }
        if (flags & MF_SOURCE_READERF_ENDOFSTREAM)
        {
            printf( "end of stream after %lu samples\n", samples );
            break;
        }
        if (!sample) continue;

        if (SUCCEEDED( IMFSample_ConvertToContiguousBuffer( sample, &buffer ) ))
        {
            BYTE *data;
            DWORD len = 0;

            if (SUCCEEDED( IMFMediaBuffer_Lock( buffer, &data, NULL, &len ) ))
            {
                DWORD i, step = len > 4096 ? len / 4096 : 1;

                checksum = 0;
                for (i = 0; i < len; i += step) checksum = checksum * 31 + data[i];
                if (samples && checksum != prev_checksum) changed++;
                prev_checksum = checksum;
                total_bytes += len;
                if (samples < 3)
                    printf( "  frame %lu: %lu bytes, ts %I64d, checksum %08x\n", samples, len, ts, checksum );
                IMFMediaBuffer_Unlock( buffer );
            }
            IMFMediaBuffer_Release( buffer );
        }
        IMFSample_Release( sample );
        samples++;
    }

    printf( "\ndecoded %lu frames, %lu bytes, %u of them differed from the previous frame\n",
            samples, total_bytes, changed );
    if (samples && changed * 4 >= samples * 3)
        printf( "RESULT: real moving video\n" );
    else if (samples)
        printf( "RESULT: frames are nearly static -- placeholder, not the file\n" );
    else
        printf( "RESULT: nothing decoded\n" );

    MFShutdown();
    return samples && changed * 4 >= samples * 3 ? 0 : 1;
}
