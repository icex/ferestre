/*
 * SRW lock / condition variable stress test.
 *
 * Written to chase a deterministic freeze in Minecraft Bedrock where three
 * separate std::mutex objects were each left with a thread parked inside
 * RtlAcquireSRWLockExclusive while the lock word said owners == 0 -- i.e. the
 * lock was free and the waiter was never woken. One of the three had a parked
 * waiter while its exclusive_waiters field read 0, which is self-contradictory:
 * a waiter adds 2 to that field before it parks.
 *
 * Wine's msvcp140 std::mutex is a plain SRWLOCK (cs_lock -> AcquireSRWLockExclusive),
 * so this hammers SRWLOCKs directly, mixing exclusive, shared, and condition
 * variable waits -- all of which manipulate the same 32-bit lock word and share
 * the same NtAlertThreadByThreadId wake path.
 *
 * A watchdog fails the run if progress stalls, and dumps each lock word so a
 * stall can be compared against the signature seen in the game.
 */
#include <windows.h>
#include <stdio.h>

#define NLOCKS   8
#define NTHREADS 24
#define NITERS   200000

struct shared
{
    SRWLOCK locks[NLOCKS];
    CONDITION_VARIABLE cv[NLOCKS];
    LONG guarded[NLOCKS];
    volatile LONG progress;
    volatile LONG done;
    volatile LONG iters[NTHREADS];
};

static struct shared g;

static DWORD WINAPI worker( void *arg )
{
    ULONG_PTR id = (ULONG_PTR)arg;
    unsigned int rng = (unsigned int)(id * 2654435761u) | 1u;
    int i;

    for (i = 0; i < NITERS; i++)
    {
        unsigned int slot;

        rng = rng * 1103515245u + 12345u;
        slot = (rng >> 16) % NLOCKS;

        switch ((rng >> 8) & 3)
        {
        case 0:
        case 1:
            AcquireSRWLockExclusive( &g.locks[slot] );
            g.guarded[slot]++;
            ReleaseSRWLockExclusive( &g.locks[slot] );
            break;
        case 2:
            AcquireSRWLockShared( &g.locks[slot] );
            (void)g.guarded[slot];
            ReleaseSRWLockShared( &g.locks[slot] );
            break;
        default:
            /* A condition variable wait releases and reacquires the SRW lock,
             * which is where exclusive/shared bookkeeping is easiest to get
             * wrong -- and it uses the same per-thread alert as the lock. */
            AcquireSRWLockExclusive( &g.locks[slot] );
            SleepConditionVariableSRW( &g.cv[slot], &g.locks[slot], 1, 0 );
            g.guarded[slot]++;
            ReleaseSRWLockExclusive( &g.locks[slot] );
            WakeAllConditionVariable( &g.cv[slot] );
            break;
        }

        InterlockedIncrement( (LONG *)&g.iters[id] );
        InterlockedIncrement( &g.progress );
    }
    InterlockedIncrement( &g.done );
    return 0;
}

static DWORD WINAPI watchdog( void *arg )
{
    LONG last = -1;
    int idle = 0;

    for (;;)
    {
        Sleep( 500 );
        if (g.done >= NTHREADS) return 0;

        if (g.progress == last)
        {
            if (++idle >= 20)   /* 10 seconds with no thread making progress */
            {
                int i;

                printf( "FAIL - stalled at %ld iterations, %ld/%d threads finished\n",
                        (long)g.progress, (long)g.done, NTHREADS );
                for (i = 0; i < NLOCKS; i++)
                {
                    unsigned int word = *(volatile unsigned int *)&g.locks[i];
                    printf( "  lock %d: raw 0x%08x  exclusive_waiters=%u (bit0=%u) owners=%u\n",
                            i, word, (word & 0xffff) >> 1, word & 1, word >> 16 );
                }
                for (i = 0; i < NTHREADS; i++)
                    printf( "  thread %2d: %ld iterations\n", i, (long)g.iters[i] );
                fflush( stdout );
                ExitProcess( 1 );
            }
        }
        else
        {
            idle = 0;
            last = g.progress;
        }
    }
}

int main( void )
{
    HANDLE threads[NTHREADS];
    ULONG_PTR i;

    for (i = 0; i < NLOCKS; i++)
    {
        InitializeSRWLock( &g.locks[i] );
        InitializeConditionVariable( &g.cv[i] );
    }

    printf( "# SRW lock stress: %d threads x %d iterations over %d locks\n",
            NTHREADS, NITERS, NLOCKS );
    fflush( stdout );

    CloseHandle( CreateThread( NULL, 0, watchdog, NULL, 0, NULL ) );
    for (i = 0; i < NTHREADS; i++)
        threads[i] = CreateThread( NULL, 0, worker, (void *)i, 0, NULL );

    for (i = 0; i < NTHREADS; i++)
    {
        if (WaitForSingleObject( threads[i], 120000 ) != WAIT_OBJECT_0)
        {
            printf( "FAIL - thread %u did not finish within the timeout\n", (unsigned)i );
            return 1;
        }
    }

    printf( "ok - all %d threads completed %d iterations each\n", NTHREADS, NITERS );
    return 0;
}
