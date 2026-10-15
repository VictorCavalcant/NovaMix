/*++

Copyright (c) Microsoft Corporation All Rights Reserved

Derived from the Windows-driver-samples SimpleAudioSample (SysVAD family),
MS-PL. Modified for the NovaMix virtual audio driver.

Module Name:

    wavtables.h

Abstract:

    Wave miniport tables for the three NovaMix endpoints.

    * NovaMix Virtual Playback: 44.1/48/96 kHz.
    * NovaMix Mic Feed / NovaMix Virtual Mic: 48 kHz only, so the internal
      loopback never needs sample-rate conversion.

    All endpoints: 16/24/32-bit PCM and 32-bit float, 1 or 2 channels.
    The first entry of each list (48 kHz, stereo, 32-bit float) is the
    default device format.
--*/

#ifndef _NOVAMIX_WAVTABLES_H_
#define _NOVAMIX_WAVTABLES_H_

#define NOVAMIX_DEVICE_MAX_CHANNELS         2

#define NOVAMIX_PLAYBACK_MIN_SAMPLE_RATE    44100
#define NOVAMIX_PLAYBACK_MAX_SAMPLE_RATE    96000
#define NOVAMIX_CABLE_SAMPLE_RATE           48000   // == LOOPBACK_SAMPLE_RATE

// Max # of pin instances. The audio engine uses one stream per endpoint.
#define NOVAMIX_MAX_RENDER_STREAMS          1
#define NOVAMIX_MAX_CAPTURE_STREAMS         1

//=============================================================================
// Device formats
//=============================================================================

#define NOVAMIX_FORMAT(SubType, Channels, Rate, Bits, Mask)                     \
    {                                                                           \
        {                                                                       \
            sizeof(KSDATAFORMAT_WAVEFORMATEXTENSIBLE),                          \
            0,                                                                  \
            0,                                                                  \
            0,                                                                  \
            STATICGUIDOF(KSDATAFORMAT_TYPE_AUDIO),                              \
            STATIC_##SubType,                                                   \
            STATICGUIDOF(KSDATAFORMAT_SPECIFIER_WAVEFORMATEX)                   \
        },                                                                      \
        {                                                                       \
            {                                                                   \
                WAVE_FORMAT_EXTENSIBLE,                                         \
                (Channels),                                                     \
                (Rate),                                                         \
                (Rate) * (Channels) * ((Bits) / 8),                             \
                (Channels) * ((Bits) / 8),                                      \
                (Bits),                                                         \
                sizeof(WAVEFORMATEXTENSIBLE) - sizeof(WAVEFORMATEX)             \
            },                                                                  \
            (Bits),                                                             \
            (Mask),                                                             \
            STATIC_##SubType                                                    \
        }                                                                       \
    }

#define NOVAMIX_FORMATS_FOR_RATE(Rate)                                                          \
    NOVAMIX_FORMAT(KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, 2, Rate, 32, KSAUDIO_SPEAKER_STEREO),      \
    NOVAMIX_FORMAT(KSDATAFORMAT_SUBTYPE_PCM,        2, Rate, 16, KSAUDIO_SPEAKER_STEREO),      \
    NOVAMIX_FORMAT(KSDATAFORMAT_SUBTYPE_PCM,        2, Rate, 24, KSAUDIO_SPEAKER_STEREO),      \
    NOVAMIX_FORMAT(KSDATAFORMAT_SUBTYPE_PCM,        2, Rate, 32, KSAUDIO_SPEAKER_STEREO),      \
    NOVAMIX_FORMAT(KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, 1, Rate, 32, KSAUDIO_SPEAKER_MONO),        \
    NOVAMIX_FORMAT(KSDATAFORMAT_SUBTYPE_PCM,        1, Rate, 16, KSAUDIO_SPEAKER_MONO),        \
    NOVAMIX_FORMAT(KSDATAFORMAT_SUBTYPE_PCM,        1, Rate, 24, KSAUDIO_SPEAKER_MONO),        \
    NOVAMIX_FORMAT(KSDATAFORMAT_SUBTYPE_PCM,        1, Rate, 32, KSAUDIO_SPEAKER_MONO)

static
KSDATAFORMAT_WAVEFORMATEXTENSIBLE PlaybackPinSupportedDeviceFormats[] =
{
    NOVAMIX_FORMATS_FOR_RATE(48000),    // first entry is the default format
    NOVAMIX_FORMATS_FOR_RATE(44100),
    NOVAMIX_FORMATS_FOR_RATE(96000),
};

static
KSDATAFORMAT_WAVEFORMATEXTENSIBLE CablePinSupportedDeviceFormats[] =
{
    NOVAMIX_FORMATS_FOR_RATE(NOVAMIX_CABLE_SAMPLE_RATE),    // first entry is the default format
};

//
// Supported modes (only on streaming pins).
//
static
MODE_AND_DEFAULT_FORMAT PlaybackPinSupportedDeviceModes[] =
{
    {
        STATIC_AUDIO_SIGNALPROCESSINGMODE_DEFAULT,
        &PlaybackPinSupportedDeviceFormats[0].DataFormat
    }
};

static
MODE_AND_DEFAULT_FORMAT MicFeedPinSupportedDeviceModes[] =
{
    {
        STATIC_AUDIO_SIGNALPROCESSINGMODE_DEFAULT,
        &CablePinSupportedDeviceFormats[0].DataFormat
    }
};

static
MODE_AND_DEFAULT_FORMAT MicPinSupportedDeviceModes[] =
{
    {
        STATIC_AUDIO_SIGNALPROCESSINGMODE_DEFAULT,
        &CablePinSupportedDeviceFormats[0].DataFormat
    },
    {
        STATIC_AUDIO_SIGNALPROCESSINGMODE_RAW,
        &CablePinSupportedDeviceFormats[0].DataFormat
    }
};

//
// The entries here must follow the same order as the filter's pin
// descriptor array.
//
static
PIN_DEVICE_FORMATS_AND_MODES PlaybackPinDeviceFormatsAndModes[] =
{
    {
        SystemRenderPin,
        PlaybackPinSupportedDeviceFormats,
        SIZEOF_ARRAY(PlaybackPinSupportedDeviceFormats),
        PlaybackPinSupportedDeviceModes,
        SIZEOF_ARRAY(PlaybackPinSupportedDeviceModes)
    },
    {
        BridgePin,
        NULL,
        0,
        NULL,
        0
    }
};

static
PIN_DEVICE_FORMATS_AND_MODES MicFeedPinDeviceFormatsAndModes[] =
{
    {
        SystemRenderPin,
        CablePinSupportedDeviceFormats,
        SIZEOF_ARRAY(CablePinSupportedDeviceFormats),
        MicFeedPinSupportedDeviceModes,
        SIZEOF_ARRAY(MicFeedPinSupportedDeviceModes)
    },
    {
        BridgePin,
        NULL,
        0,
        NULL,
        0
    }
};

static
PIN_DEVICE_FORMATS_AND_MODES MicPinDeviceFormatsAndModes[] =
{
    {
        BridgePin,
        NULL,
        0,
        NULL,
        0
    },
    {
        SystemCapturePin,
        CablePinSupportedDeviceFormats,
        SIZEOF_ARRAY(CablePinSupportedDeviceFormats),
        MicPinSupportedDeviceModes,
        SIZEOF_ARRAY(MicPinSupportedDeviceModes)
    }
};

//=============================================================================
// Data ranges. One range per (sample type, channel count): see
// CMiniportWaveRT::DataRangeIntersection. Every range is followed by the
// signal processing mode attribute list.
//=============================================================================

#define NOVAMIX_RANGE(SubType, Channels, MinBits, MaxBits, MinRate, MaxRate)    \
    {                                                                           \
        {                                                                       \
            sizeof(KSDATARANGE_AUDIO),                                          \
            KSDATARANGE_ATTRIBUTES,                                             \
            0,                                                                  \
            0,                                                                  \
            STATICGUIDOF(KSDATAFORMAT_TYPE_AUDIO),                              \
            STATIC_##SubType,                                                   \
            STATICGUIDOF(KSDATAFORMAT_SPECIFIER_WAVEFORMATEX)                   \
        },                                                                      \
        (Channels),                                                             \
        (MinBits),                                                              \
        (MaxBits),                                                              \
        (MinRate),                                                              \
        (MaxRate)                                                               \
    }

static
KSDATARANGE_AUDIO PlaybackPinDataRangesStream[] =
{
    NOVAMIX_RANGE(KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, 2, 32, 32, NOVAMIX_PLAYBACK_MIN_SAMPLE_RATE, NOVAMIX_PLAYBACK_MAX_SAMPLE_RATE),
    NOVAMIX_RANGE(KSDATAFORMAT_SUBTYPE_PCM,        2, 16, 32, NOVAMIX_PLAYBACK_MIN_SAMPLE_RATE, NOVAMIX_PLAYBACK_MAX_SAMPLE_RATE),
    NOVAMIX_RANGE(KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, 1, 32, 32, NOVAMIX_PLAYBACK_MIN_SAMPLE_RATE, NOVAMIX_PLAYBACK_MAX_SAMPLE_RATE),
    NOVAMIX_RANGE(KSDATAFORMAT_SUBTYPE_PCM,        1, 16, 32, NOVAMIX_PLAYBACK_MIN_SAMPLE_RATE, NOVAMIX_PLAYBACK_MAX_SAMPLE_RATE),
};

static
PKSDATARANGE PlaybackPinDataRangePointersStream[] =
{
    PKSDATARANGE(&PlaybackPinDataRangesStream[0]),
    PKSDATARANGE(&PinDataRangeAttributeList),
    PKSDATARANGE(&PlaybackPinDataRangesStream[1]),
    PKSDATARANGE(&PinDataRangeAttributeList),
    PKSDATARANGE(&PlaybackPinDataRangesStream[2]),
    PKSDATARANGE(&PinDataRangeAttributeList),
    PKSDATARANGE(&PlaybackPinDataRangesStream[3]),
    PKSDATARANGE(&PinDataRangeAttributeList),
};

static
KSDATARANGE_AUDIO CablePinDataRangesStream[] =
{
    NOVAMIX_RANGE(KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, 2, 32, 32, NOVAMIX_CABLE_SAMPLE_RATE, NOVAMIX_CABLE_SAMPLE_RATE),
    NOVAMIX_RANGE(KSDATAFORMAT_SUBTYPE_PCM,        2, 16, 32, NOVAMIX_CABLE_SAMPLE_RATE, NOVAMIX_CABLE_SAMPLE_RATE),
    NOVAMIX_RANGE(KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, 1, 32, 32, NOVAMIX_CABLE_SAMPLE_RATE, NOVAMIX_CABLE_SAMPLE_RATE),
    NOVAMIX_RANGE(KSDATAFORMAT_SUBTYPE_PCM,        1, 16, 32, NOVAMIX_CABLE_SAMPLE_RATE, NOVAMIX_CABLE_SAMPLE_RATE),
};

static
PKSDATARANGE CablePinDataRangePointersStream[] =
{
    PKSDATARANGE(&CablePinDataRangesStream[0]),
    PKSDATARANGE(&PinDataRangeAttributeList),
    PKSDATARANGE(&CablePinDataRangesStream[1]),
    PKSDATARANGE(&PinDataRangeAttributeList),
    PKSDATARANGE(&CablePinDataRangesStream[2]),
    PKSDATARANGE(&PinDataRangeAttributeList),
    PKSDATARANGE(&CablePinDataRangesStream[3]),
    PKSDATARANGE(&PinDataRangeAttributeList),
};

//=============================================================================
static
KSDATARANGE WavePinDataRangesBridge[] =
{
    {
        sizeof(KSDATARANGE),
        0,
        0,
        0,
        STATICGUIDOF(KSDATAFORMAT_TYPE_AUDIO),
        STATICGUIDOF(KSDATAFORMAT_SUBTYPE_ANALOG),
        STATICGUIDOF(KSDATAFORMAT_SPECIFIER_NONE)
    }
};

static
PKSDATARANGE WavePinDataRangePointersBridge[] =
{
    &WavePinDataRangesBridge[0]
};

//=============================================================================
// Wave filter automation (same for the three endpoints).
//=============================================================================
static
PCPROPERTY_ITEM PropertiesWaveFilter[] =
{
    {
        &KSPROPSETID_Pin,
        KSPROPERTY_PIN_PROPOSEDATAFORMAT,
        KSPROPERTY_TYPE_SET | KSPROPERTY_TYPE_BASICSUPPORT,
        PropertyHandler_WaveFilter
    },
    {
        &KSPROPSETID_Pin,
        KSPROPERTY_PIN_PROPOSEDATAFORMAT2,
        KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_BASICSUPPORT,
        PropertyHandler_WaveFilter
    }
};

DEFINE_PCAUTOMATION_TABLE_PROP(AutomationWaveFilter, PropertiesWaveFilter);

//=============================================================================
// Render wave filters (Virtual Playback, Mic Feed)
//
//                   ----------------------------
//                   |                          |
//  Host Pin     0-->|                          |--> 1 KSPIN_WAVE_RENDER_SOURCE
//                   |                          |
//                   ----------------------------
//=============================================================================
#define NOVAMIX_RENDER_WAVE_PINS(StreamRanges)                                  \
    {                                                                           \
        /* KSPIN_WAVE_RENDER_SINK_SYSTEM */                                     \
        {                                                                       \
            NOVAMIX_MAX_RENDER_STREAMS,                                         \
            NOVAMIX_MAX_RENDER_STREAMS,                                         \
            0,                                                                  \
            NULL,                                                               \
            {                                                                   \
                0,                                                              \
                NULL,                                                           \
                0,                                                              \
                NULL,                                                           \
                SIZEOF_ARRAY(StreamRanges),                                     \
                StreamRanges,                                                   \
                KSPIN_DATAFLOW_IN,                                              \
                KSPIN_COMMUNICATION_SINK,                                       \
                &KSCATEGORY_AUDIO,                                              \
                NULL,                                                           \
                0                                                               \
            }                                                                   \
        },                                                                      \
        /* KSPIN_WAVE_RENDER_SOURCE */                                          \
        {                                                                       \
            0,                                                                  \
            0,                                                                  \
            0,                                                                  \
            NULL,                                                               \
            {                                                                   \
                0,                                                              \
                NULL,                                                           \
                0,                                                              \
                NULL,                                                           \
                SIZEOF_ARRAY(WavePinDataRangePointersBridge),                   \
                WavePinDataRangePointersBridge,                                 \
                KSPIN_DATAFLOW_OUT,                                             \
                KSPIN_COMMUNICATION_NONE,                                       \
                &KSCATEGORY_AUDIO,                                              \
                NULL,                                                           \
                0                                                               \
            }                                                                   \
        },                                                                      \
    }

static
PCPIN_DESCRIPTOR PlaybackWaveMiniportPins[] =
    NOVAMIX_RENDER_WAVE_PINS(PlaybackPinDataRangePointersStream);

static
PCPIN_DESCRIPTOR MicFeedWaveMiniportPins[] =
    NOVAMIX_RENDER_WAVE_PINS(CablePinDataRangePointersStream);

static
PCCONNECTION_DESCRIPTOR RenderWaveMiniportConnections[] =
{
    { PCFILTER_NODE,    KSPIN_WAVE_RENDER_SINK_SYSTEM,  PCFILTER_NODE,  KSPIN_WAVE_RENDER_SOURCE }
};

static
PCFILTER_DESCRIPTOR PlaybackWaveMiniportFilterDescriptor =
{
    0,                                              // Version
    &AutomationWaveFilter,                          // AutomationTable
    sizeof(PCPIN_DESCRIPTOR),                       // PinSize
    SIZEOF_ARRAY(PlaybackWaveMiniportPins),         // PinCount
    PlaybackWaveMiniportPins,                       // Pins
    sizeof(PCNODE_DESCRIPTOR),                      // NodeSize
    0,                                              // NodeCount
    NULL,                                           // Nodes
    SIZEOF_ARRAY(RenderWaveMiniportConnections),    // ConnectionCount
    RenderWaveMiniportConnections,                  // Connections
    0,                                              // CategoryCount
    NULL                                            // Categories  - use defaults (audio, render, capture)
};

static
PCFILTER_DESCRIPTOR MicFeedWaveMiniportFilterDescriptor =
{
    0,                                              // Version
    &AutomationWaveFilter,                          // AutomationTable
    sizeof(PCPIN_DESCRIPTOR),                       // PinSize
    SIZEOF_ARRAY(MicFeedWaveMiniportPins),          // PinCount
    MicFeedWaveMiniportPins,                        // Pins
    sizeof(PCNODE_DESCRIPTOR),                      // NodeSize
    0,                                              // NodeCount
    NULL,                                           // Nodes
    SIZEOF_ARRAY(RenderWaveMiniportConnections),    // ConnectionCount
    RenderWaveMiniportConnections,                  // Connections
    0,                                              // CategoryCount
    NULL                                            // Categories  - use defaults (audio, render, capture)
};

//=============================================================================
// Capture wave filter (Virtual Mic)
//=============================================================================
static
PCPIN_DESCRIPTOR MicWaveMiniportPins[] =
{
    // Wave In Bridge Pin (Capture - From Topology) KSPIN_WAVE_BRIDGE
    {
        0,
        0,
        0,
        NULL,
        {
            0,
            NULL,
            0,
            NULL,
            SIZEOF_ARRAY(WavePinDataRangePointersBridge),
            WavePinDataRangePointersBridge,
            KSPIN_DATAFLOW_IN,
            KSPIN_COMMUNICATION_NONE,
            &KSCATEGORY_AUDIO,
            NULL,
            0
        }
    },
    // Wave In Streaming Pin (Capture) KSPIN_WAVEIN_HOST
    {
        NOVAMIX_MAX_CAPTURE_STREAMS,
        NOVAMIX_MAX_CAPTURE_STREAMS,
        0,
        NULL,
        {
            0,
            NULL,
            0,
            NULL,
            SIZEOF_ARRAY(CablePinDataRangePointersStream),
            CablePinDataRangePointersStream,
            KSPIN_DATAFLOW_OUT,
            KSPIN_COMMUNICATION_SINK,
            &KSCATEGORY_AUDIO,
            &KSAUDFNAME_RECORDING_CONTROL,
            0
        }
    }
};

static
PCNODE_DESCRIPTOR MicWaveMiniportNodes[] =
{
    // KSNODE_WAVE_ADC
    {
        0,                      // Flags
        NULL,                   // AutomationTable
        &KSNODETYPE_ADC,        // Type
        NULL                    // Name
    }
};

static
PCCONNECTION_DESCRIPTOR MicWaveMiniportConnections[] =
{
    { PCFILTER_NODE,        KSPIN_WAVE_BRIDGE,      KSNODE_WAVE_ADC,     1 },
    { KSNODE_WAVE_ADC,      0,                      PCFILTER_NODE,       KSPIN_WAVEIN_HOST },
};

static
PCFILTER_DESCRIPTOR MicWaveMiniportFilterDescriptor =
{
    0,                                              // Version
    &AutomationWaveFilter,                          // AutomationTable
    sizeof(PCPIN_DESCRIPTOR),                       // PinSize
    SIZEOF_ARRAY(MicWaveMiniportPins),              // PinCount
    MicWaveMiniportPins,                            // Pins
    sizeof(PCNODE_DESCRIPTOR),                      // NodeSize
    SIZEOF_ARRAY(MicWaveMiniportNodes),             // NodeCount
    MicWaveMiniportNodes,                           // Nodes
    SIZEOF_ARRAY(MicWaveMiniportConnections),       // ConnectionCount
    MicWaveMiniportConnections,                     // Connections
    0,                                              // CategoryCount
    NULL                                            // Categories  - use defaults (audio, render, capture)
};

#endif // _NOVAMIX_WAVTABLES_H_
