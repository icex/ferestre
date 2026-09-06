/*
 * Is ECDH on P-384 through CNG correct here?
 *
 * Bedrock agrees its packet-encryption key with the server by ECDH on P-384,
 * then derives the AES key from the raw shared secret. A wrong secret produces
 * a wrong key, and the client then fails to make sense of anything the server
 * sends -- while RakNet, which is outside the encryption, keeps acknowledging
 * happily. That is indistinguishable from a healthy connection that never
 * progresses, so the secret is worth checking rather than assuming.
 *
 * Generates two key pairs, prints both private keys and the shared secret so a
 * reference implementation can recompute it. The keys are ephemeral test keys.
 *
 * Build: x86_64-w64-mingw32-gcc -O2 -o ecdh_probe.exe ecdh_probe.c -lbcrypt
 */
#include <windows.h>
#include <bcrypt.h>
#include <stdio.h>

static void dump( const char *name, const UCHAR *p, ULONG len )
{
    ULONG i;
    printf( "%s ", name );
    for (i = 0; i < len; i++) printf( "%02x", p[i] );
    printf( "\n" );
}

int main( void )
{
    BCRYPT_ALG_HANDLE alg = NULL;
    BCRYPT_KEY_HANDLE a = NULL, b = NULL, bpub = NULL;
    BCRYPT_SECRET_HANDLE secret = NULL;
    UCHAR blob_a[512], blob_b[512], pub_b[512], out[128];
    ULONG len_a = 0, len_b = 0, len_pub = 0, out_len = 0;
    NTSTATUS st;

    if ((st = BCryptOpenAlgorithmProvider( &alg, BCRYPT_ECDH_P384_ALGORITHM, NULL, 0 )))
    { printf( "open P-384 failed %#lx\n", (long)st ); return 2; }

    if ((st = BCryptGenerateKeyPair( alg, &a, 384, 0 )) || (st = BCryptFinalizeKeyPair( a, 0 )))
    { printf( "keypair A failed %#lx\n", (long)st ); return 2; }
    if ((st = BCryptGenerateKeyPair( alg, &b, 384, 0 )) || (st = BCryptFinalizeKeyPair( b, 0 )))
    { printf( "keypair B failed %#lx\n", (long)st ); return 2; }

    if ((st = BCryptExportKey( a, NULL, BCRYPT_ECCPRIVATE_BLOB, blob_a, sizeof(blob_a), &len_a, 0 )))
    { printf( "export A failed %#lx\n", (long)st ); return 2; }
    if ((st = BCryptExportKey( b, NULL, BCRYPT_ECCPRIVATE_BLOB, blob_b, sizeof(blob_b), &len_b, 0 )))
    { printf( "export B failed %#lx\n", (long)st ); return 2; }
    if ((st = BCryptExportKey( b, NULL, BCRYPT_ECCPUBLIC_BLOB, pub_b, sizeof(pub_b), &len_pub, 0 )))
    { printf( "export B public failed %#lx\n", (long)st ); return 2; }

    if ((st = BCryptImportKeyPair( alg, NULL, BCRYPT_ECCPUBLIC_BLOB, &bpub, pub_b, len_pub, 0 )))
    { printf( "import B public failed %#lx\n", (long)st ); return 2; }

    if ((st = BCryptSecretAgreement( a, bpub, &secret, 0 )))
    { printf( "SecretAgreement failed %#lx\n", (long)st ); return 3; }

    if ((st = BCryptDeriveKey( secret, BCRYPT_KDF_RAW_SECRET, NULL, out, sizeof(out), &out_len, 0 )))
    { printf( "DeriveKey(RAW_SECRET) failed %#lx\n", (long)st ); return 3; }

    /* blobs are BCRYPT_ECCKEY_BLOB: magic, cbKey, then X, Y, and (private) d */
    {
        ULONG cb = *(ULONG *)(blob_a + 4);
        dump( "A_d", blob_a + 8 + 2 * cb, cb );
        dump( "B_x", blob_b + 8, cb );
        dump( "B_y", blob_b + 8 + cb, cb );
        dump( "secret", out, out_len );
    }
    return 0;
}
