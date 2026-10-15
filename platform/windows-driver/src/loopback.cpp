/*++

Copyright (c) NovaMix contributors

Module Name:

    loopback.cpp

Abstract:

    Mic Feed -> Virtual Mic ring buffer. See loopback.h.
--*/

#pragma warning (disable : 4127)

#include "definitions.h"
#include "loopback.h"

#ifndef WAVE_FORMAT_IEEE_FLOAT
#define WAVE_FORMAT_IEEE_FLOAT 0x0003
#endif

#define LOOPBACK_POOLTAG            'bLvN'

CLoopbackRing g_MicLoopback;

// Reader latency in ms; can be overridden by the MicLoopbackLatencyMs
// registry value (see adapter.cpp).
DWORD g_MicLoopbackLatencyMs = LOOPBACK_TARGET_MS;

//=============================================================================
#pragma code_seg("PAGE")
NTSTATUS
LoopbackFormatFromWaveFormat
(
    _In_  PWAVEFORMATEX     WaveFormat,
    _Out_ PLOOPBACK_FORMAT  Format
)
{
    PAGED_CODE();

    BOOLEAN isFloat;
    USHORT  containerBytes = (USHORT)(WaveFormat->wBitsPerSample / 8);

    RtlZeroMemory(Format, sizeof(*Format));

    if (WaveFormat->wFormatTag == WAVE_FORMAT_EXTENSIBLE)
    {
        if (WaveFormat->cbSize < sizeof(WAVEFORMATEXTENSIBLE) - sizeof(WAVEFORMATEX))
        {
            return STATUS_NOT_SUPPORTED;
        }

        PWAVEFORMATEXTENSIBLE ext = reinterpret_cast<PWAVEFORMATEXTENSIBLE>(WaveFormat);
        if (IsEqualGUID(ext->SubFormat, KSDATAFORMAT_SUBTYPE_IEEE_FLOAT))
        {
            isFloat = TRUE;
        }
        else if (IsEqualGUID(ext->SubFormat, KSDATAFORMAT_SUBTYPE_PCM))
        {
            isFloat = FALSE;
        }
        else
        {
            return STATUS_NOT_SUPPORTED;
        }
    }
    else if (WaveFormat->wFormatTag == WAVE_FORMAT_IEEE_FLOAT)
    {
        isFloat = TRUE;
    }
    else if (WaveFormat->wFormatTag == WAVE_FORMAT_PCM)
    {
        isFloat = FALSE;
    }
    else
    {
        return STATUS_NOT_SUPPORTED;
    }

    if (WaveFormat->nChannels < 1 || WaveFormat->nChannels > 2)
    {
        return STATUS_NOT_SUPPORTED;
    }

    if (isFloat ? (containerBytes != 4) : (containerBytes < 2 || containerBytes > 4))
    {
        return STATUS_NOT_SUPPORTED;
    }

    if (WaveFormat->nBlockAlign != WaveFormat->nChannels * containerBytes)
    {
        return STATUS_NOT_SUPPORTED;
    }

    Format->Channels       = WaveFormat->nChannels;
    Format->ContainerBytes = containerBytes;
    Format->IsFloat        = isFloat;
    Format->BlockAlign     = WaveFormat->nBlockAlign;

    return STATUS_SUCCESS;
}

//=============================================================================
#pragma code_seg("PAGE")
_Use_decl_annotations_
NTSTATUS
CLoopbackRing::Initialize()
{
    PAGED_CODE();

    KeInitializeSpinLock(&m_Lock);

    m_Capacity  = LOOPBACK_SAMPLE_RATE / 1000 * LOOPBACK_CAPACITY_MS;
    m_WritePos  = 0;
    m_ReadPos   = 0;
    m_Primed    = FALSE;
    m_Underruns = 0;
    m_Overruns  = 0;

    // ExAllocatePool2 zeroes the allocation: the ring starts silent.
    m_Buffer = (FLOAT *)ExAllocatePool2(
        POOL_FLAG_NON_PAGED,
        (SIZE_T)m_Capacity * 2 * sizeof(FLOAT),
        LOOPBACK_POOLTAG);

    return (m_Buffer != NULL) ? STATUS_SUCCESS : STATUS_INSUFFICIENT_RESOURCES;
}

//=============================================================================
#pragma code_seg("PAGE")
_Use_decl_annotations_
VOID
CLoopbackRing::Cleanup()
{
    PAGED_CODE();

    if (m_Buffer != NULL)
    {
        ExFreePoolWithTag(m_Buffer, LOOPBACK_POOLTAG);
        m_Buffer = NULL;
    }

    DPF(D_TERSE, ("[Loopback] underruns=%u overruns=%u", m_Underruns, m_Overruns));
}

//=============================================================================
#pragma code_seg()

static __forceinline
FLOAT
ReadSample
(
    _In_ const BYTE *       Source,
    _In_ PLOOPBACK_FORMAT   Format
)
{
    FLOAT value;

    if (Format->IsFloat)
    {
        RtlCopyMemory(&value, Source, sizeof(value));
        if (value != value)
        {
            value = 0.0f;   // NaN
        }
        return value;
    }

    switch (Format->ContainerBytes)
    {
        case 2:
        {
            SHORT s;
            RtlCopyMemory(&s, Source, sizeof(s));
            value = s * (1.0f / 32768.0f);
            break;
        }
        case 3:
        {
            LONG s = (LONG)(((ULONG)Source[0] << 8) | ((ULONG)Source[1] << 16) | ((ULONG)Source[2] << 24)) >> 8;
            value = s * (1.0f / 8388608.0f);
            break;
        }
        default:
        {
            LONG s;
            RtlCopyMemory(&s, Source, sizeof(s));
            value = (FLOAT)((double)s * (1.0 / 2147483648.0));
            break;
        }
    }

    return value;
}

static __forceinline
VOID
WriteSample
(
    _Out_writes_bytes_(Format->ContainerBytes) BYTE *  Destination,
    _In_ FLOAT                                          Value,
    _In_ PLOOPBACK_FORMAT                               Format
)
{
    if (Format->IsFloat)
    {
        RtlCopyMemory(Destination, &Value, sizeof(Value));
        return;
    }

    if (Value > 1.0f)
    {
        Value = 1.0f;
    }
    else if (Value < -1.0f)
    {
        Value = -1.0f;
    }

    switch (Format->ContainerBytes)
    {
        case 2:
        {
            SHORT s = (SHORT)(Value * 32767.0f);
            RtlCopyMemory(Destination, &s, sizeof(s));
            break;
        }
        case 3:
        {
            LONG s = (LONG)(Value * 8388607.0f);
            Destination[0] = (BYTE)(s);
            Destination[1] = (BYTE)(s >> 8);
            Destination[2] = (BYTE)(s >> 16);
            break;
        }
        default:
        {
            LONG s = (LONG)((double)Value * 2147483647.0);
            RtlCopyMemory(Destination, &s, sizeof(s));
            break;
        }
    }
}

//=============================================================================
#pragma code_seg()
_Use_decl_annotations_
VOID
CLoopbackRing::ResetReader()
{
    KIRQL oldIrql;

    KeAcquireSpinLock(&m_Lock, &oldIrql);
    m_Primed = FALSE;
    KeReleaseSpinLock(&m_Lock, oldIrql);
}

//=============================================================================
#pragma code_seg()
_Use_decl_annotations_
VOID
CLoopbackRing::Write
(
    const BYTE *        Source,
    ULONG               Frames,
    PLOOPBACK_FORMAT    Format
)
{
    KFLOATING_SAVE  floatSave;
    KIRQL           oldIrql;

    if (m_Buffer == NULL || Frames == 0)
    {
        return;
    }

    if (!NT_SUCCESS(KeSaveFloatingPointState(&floatSave)))
    {
        return;
    }

    KeAcquireSpinLock(&m_Lock, &oldIrql);

    // Only the newest m_Capacity frames can survive in the ring.
    if (Frames > m_Capacity)
    {
        Source     += (SIZE_T)(Frames - m_Capacity) * Format->BlockAlign;
        m_WritePos += Frames - m_Capacity;
        Frames      = m_Capacity;
    }

    ULONG index = (ULONG)(m_WritePos % m_Capacity);

    for (ULONG i = 0; i < Frames; i++)
    {
        FLOAT left  = ReadSample(Source, Format);
        FLOAT right = (Format->Channels > 1) ? ReadSample(Source + Format->ContainerBytes, Format) : left;

        m_Buffer[2 * index]     = left;
        m_Buffer[2 * index + 1] = right;

        Source += Format->BlockAlign;
        if (++index == m_Capacity)
        {
            index = 0;
        }
    }

    m_WritePos += Frames;

    if (m_WritePos - m_ReadPos > m_Capacity)
    {
        // Oldest frames were overwritten. Only an overrun if a reader is
        // actually consuming; otherwise the ring is just idling.
        m_ReadPos = m_WritePos - m_Capacity;
        if (m_Primed)
        {
            m_Overruns++;
        }
    }

    KeReleaseSpinLock(&m_Lock, oldIrql);

    KeRestoreFloatingPointState(&floatSave);
}

//=============================================================================
#pragma code_seg()
_Use_decl_annotations_
VOID
CLoopbackRing::Read
(
    BYTE *              Destination,
    ULONG               Frames,
    PLOOPBACK_FORMAT    Format
)
{
    KFLOATING_SAVE  floatSave;
    KIRQL           oldIrql;
    ULONG           copied = 0;

    if (Frames == 0)
    {
        return;
    }

    if (m_Buffer == NULL || !NT_SUCCESS(KeSaveFloatingPointState(&floatSave)))
    {
        RtlZeroMemory(Destination, (SIZE_T)Frames * Format->BlockAlign);
        return;
    }

    KeAcquireSpinLock(&m_Lock, &oldIrql);

    ULONG       target    = LOOPBACK_SAMPLE_RATE / 1000 * g_MicLoopbackLatencyMs;
    ULONG       maxFill   = target + LOOPBACK_SAMPLE_RATE / 1000 * (LOOPBACK_MAX_MS - LOOPBACK_TARGET_MS);
    ULONGLONG   available = m_WritePos - m_ReadPos;

    if (!m_Primed)
    {
        // Wait for a cushion of `target` frames, then start from there.
        if (available >= target)
        {
            m_Primed  = TRUE;
            m_ReadPos = m_WritePos - target;
            available = target;
        }
        else
        {
            available = 0;
        }
    }
    else if (available > maxFill)
    {
        // The reader fell behind (e.g. it was paused): drop back to the target latency.
        m_ReadPos = m_WritePos - target;
        available = target;
    }

    copied = (ULONG)min(available, (ULONGLONG)Frames);

    ULONG index = (ULONG)(m_ReadPos % m_Capacity);

    for (ULONG i = 0; i < copied; i++)
    {
        FLOAT left  = m_Buffer[2 * index];
        FLOAT right = m_Buffer[2 * index + 1];

        if (Format->Channels > 1)
        {
            WriteSample(Destination, left, Format);
            WriteSample(Destination + Format->ContainerBytes, right, Format);
        }
        else
        {
            WriteSample(Destination, (left + right) * 0.5f, Format);
        }

        Destination += Format->BlockAlign;
        if (++index == m_Capacity)
        {
            index = 0;
        }
    }

    m_ReadPos += copied;

    if (copied < Frames)
    {
        if (m_Primed)
        {
            // Underrun: deliver silence and wait for a new cushion.
            m_Primed = FALSE;
            m_Underruns++;
            DPF(D_VERBOSE, ("[Loopback] underrun #%u", m_Underruns));
        }
        RtlZeroMemory(Destination, (SIZE_T)(Frames - copied) * Format->BlockAlign);
    }

    KeReleaseSpinLock(&m_Lock, oldIrql);

    KeRestoreFloatingPointState(&floatSave);
}
