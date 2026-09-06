/*
 * Is SHA-256 through CNG correct here?
 *
 * Every encrypted Bedrock packet carries an eight-byte checksum, the first
 * bytes of SHA-256 over a counter, the plaintext and the key. A client whose
 * hash disagrees with the server's discards every packet it receives without
 * saying so, which looks exactly like a connection that is up and idle.
 *
 * Build: x86_64-w64-mingw32-gcc -O2 -o sha256_probe.exe sha256_probe.c -lbcrypt
 */
#include <windows.h>
#include <bcrypt.h>
#include <stdio.h>
#include <string.h>

static int digest( const WCHAR *algo, const char *msg, const char *label )
{
    BCRYPT_ALG_HANDLE alg = NULL;
    BCRYPT_HASH_HANDLE h = NULL;
    UCHAR out[64];
    ULONG len = 0, i, objlen = 0, cb = 0;
    UCHAR *obj = NULL;
    NTSTATUS st;

    if ((st = BCryptOpenAlgorithmProvider( &alg, algo, NULL, 0 ))) { printf( "%s: open failed %#lx\n", label, (long)st ); return 1; }
    BCryptGetProperty( alg, BCRYPT_OBJECT_LENGTH, (UCHAR *)&objlen, sizeof(objlen), &cb, 0 );
    BCryptGetProperty( alg, BCRYPT_HASH_LENGTH, (UCHAR *)&len, sizeof(len), &cb, 0 );
    if (objlen) obj = HeapAlloc( GetProcessHeap(), 0, objlen );
    if ((st = BCryptCreateHash( alg, &h, obj, objlen, NULL, 0, 0 ))) { printf( "%s: create failed %#lx\n", label, (long)st ); return 1; }
    if ((st = BCryptHashData( h, (UCHAR *)msg, (ULONG)strlen( msg ), 0 ))) { printf( "%s: hash failed %#lx\n", label, (long)st ); return 1; }
    if ((st = BCryptFinishHash( h, out, len, 0 ))) { printf( "%s: finish failed %#lx\n", label, (long)st ); return 1; }

    printf( "%s ", label );
    for (i = 0; i < len; i++) printf( "%02x", out[i] );
    printf( "\n" );
    return 0;
}

int main( void )
{
    digest( BCRYPT_SHA256_ALGORITHM, "abc", "sha256(abc)" );
    digest( BCRYPT_SHA256_ALGORITHM, "", "sha256(empty)" );
    digest( BCRYPT_SHA1_ALGORITHM,   "abc", "sha1(abc)" );
    digest( BCRYPT_SHA512_ALGORITHM, "abc", "sha512(abc)" );
    return 0;
}
