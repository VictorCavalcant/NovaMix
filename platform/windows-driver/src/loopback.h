/*++

Copyright (c) NovaMix contributors

Module Name:

    loopback.h

Abstract:

    The single internal "cable" of the NovaMix driver: a ring buffer that
    carries the audio rendered to "NovaMix Mic Feed" to the capture endpoint
    "NovaMix Virtual Mic".

    Both endpoints are clocked by the same system timer, so there is no
    long-term drift between writer and reader; the ring only absorbs the
    jitter of the timer DPCs. Samples are stored as interleaved 32-bit float
    stereo, so the two endpoints may use different channel counts and sample
    types. Both endpoints only expose 48 kHz, so no rate conversion is needed
    in kernel mode (NovaMix resamples in user mode).

    Writer and reader run at DISPATCH_LEVEL (timer DPC / position queries).
--*/

#ifndef _NOVAMIX_LOOPBACK_H_
#define _NOVAMIX_LOOPBACK_H_

#define LOOPBACK_SAMPLE_RATE        48000
#define LOOPBACK_CAPACITY_MS        200     // ring size
#define LOOPBACK_TARGET_MS          20      // reader latency after (re)priming
#define LOOPBACK_MAX_MS             100     // above this the reader drops to the target

//
// Sample layout of one side (Mic Feed or Virtual Mic) of the loopback.
//
typedef struct _LOOPBACK_FORMAT
{
    USHORT  Channels;           // 1 or 2
    USHORT  ContainerBytes;     // 2, 3 or 4
    BOOLEAN IsFloat;            // 32-bit IEEE float
    ULONG   BlockAlign;         // Channels * ContainerBytes
} LOOPBACK_FORMAT, *PLOOPBACK_FORMAT;

NTSTATUS LoopbackFormatFromWaveFormat
(
    _In_  PWAVEFORMATEX     WaveFormat,
    _Out_ PLOOPBACK_FORMAT  Format
);

//
// No constructor on purpose: the only instance is a global that is
// explicitly initialized in DriverEntry and freed in DriverUnload, so it
// outlives every stream.
//
class CLoopbackRing
{
public:
    _IRQL_requires_(PASSIVE_LEVEL)
    NTSTATUS    Initialize();

    _IRQL_requires_(PASSIVE_LEVEL)
    VOID        Cleanup();

    // Called when the Virtual Mic stream (re)starts: the reader waits for
    // LOOPBACK_TARGET_MS of data before delivering it.
    _IRQL_requires_max_(DISPATCH_LEVEL)
    VOID        ResetReader();

    _IRQL_requires_max_(DISPATCH_LEVEL)
    VOID        Write
    (
        _In_reads_bytes_(Frames * Format->BlockAlign) const BYTE *  Source,
        _In_                                          ULONG         Frames,
        _In_                                          PLOOPBACK_FORMAT Format
    );

    // Always fills Frames frames; missing data is replaced by silence.
    _IRQL_requires_max_(DISPATCH_LEVEL)
    VOID        Read
    (
        _Out_writes_bytes_(Frames * Format->BlockAlign) BYTE *      Destination,
        _In_                                            ULONG       Frames,
        _In_                                            PLOOPBACK_FORMAT Format
    );

private:
    KSPIN_LOCK  m_Lock;
    FLOAT *     m_Buffer;           // m_Capacity frames of interleaved L/R
    ULONG       m_Capacity;         // frames
    ULONGLONG   m_WritePos;         // total frames written
    ULONGLONG   m_ReadPos;          // total frames consumed (<= m_WritePos)
    BOOLEAN     m_Primed;
    ULONG       m_Underruns;
    ULONG       m_Overruns;
};

extern CLoopbackRing g_MicLoopback;

// Reader latency (ms), from the MicLoopbackLatencyMs registry value.
extern DWORD g_MicLoopbackLatencyMs;

#endif // _NOVAMIX_LOOPBACK_H_
