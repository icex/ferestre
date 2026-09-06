/*
 * Is AES-CFB8 through CNG correct here?
 *
 * Minecraft Bedrock encrypts its packet stream with AES-256-CFB8 once login
 * completes, using bcrypt on Windows. RakNet's acknowledgements are outside
 * that encryption, so a wrong cipher looks exactly like a healthy connection
 * that makes no progress: datagrams arrive, get acknowledged, and nothing in
 * the game advances. This encrypts a known vector so the result can be
 * compared with a reference.
 *
 * Build: x86_64-w64-mingw32-gcc -O2 -o aes_cfb8_probe.exe aes_cfb8_probe.c -lbcrypt
 */
#include <windows.h>
#include <bcrypt.h>
#include <stdio.h>

int main( void )
{
    BCRYPT_ALG_HANDLE alg = NULL;
    BCRYPT_KEY_HANDLE key = NULL;
    UCHAR keybytes[32], iv[16], data[32], out[32];
    ULONG i, done = 0;
    NTSTATUS st;

    for (i = 0; i < 32; i++) keybytes[i] = (UCHAR)i;          /* 00..1f */
    for (i = 0; i < 16; i++) iv[i] = (UCHAR)(0xf0 + i);       /* f0..ff */
    for (i = 0; i < 32; i++) data[i] = (UCHAR)(0xa0 + i);     /* a0..bf */

    if ((st = BCryptOpenAlgorithmProvider( &alg, BCRYPT_AES_ALGORITHM, NULL, 0 )))
    { printf( "OpenAlgorithmProvider failed %#lx\n", (long)st ); return 2; }

    if ((st = BCryptSetProperty( alg, BCRYPT_CHAINING_MODE, (UCHAR *)BCRYPT_CHAIN_MODE_CFB,
                                 sizeof(BCRYPT_CHAIN_MODE_CFB), 0 )))
    { printf( "chaining mode CFB rejected %#lx\n", (long)st ); return 3; }

    if ((st = BCryptGenerateSymmetricKey( alg, &key, NULL, 0, keybytes, sizeof(keybytes), 0 )))
    { printf( "GenerateSymmetricKey failed %#lx\n", (long)st ); return 2; }

    if ((st = BCryptEncrypt( key, data, sizeof(data), NULL, iv, sizeof(iv),
                             out, sizeof(out), &done, 0 )))
    { printf( "BCryptEncrypt failed %#lx\n", (long)st ); return 2; }

    printf( "cfb8 %lu bytes: ", done );
    for (i = 0; i < done; i++) printf( "%02x", out[i] );
    printf( "\n" );
    return 0;
}
