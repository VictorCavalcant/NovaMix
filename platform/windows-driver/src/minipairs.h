/*++

Copyright (c) Microsoft Corporation All Rights Reserved

Derived from the Windows-driver-samples SimpleAudioSample (SysVAD family),
MS-PL. Modified for the NovaMix virtual audio driver.

Module Name:

    minipairs.h

Abstract:

    The three NovaMix endpoints (wave + topology filter pairs).

    The filter reference strings (TopoName/WaveName) must match the KSNAME_*
    strings of the INF.
--*/

#ifndef _NOVAMIX_MINIPAIRS_H_
#define _NOVAMIX_MINIPAIRS_H_

#include "toptables.h"
#include "wavtables.h"

NTSTATUS
CreateMiniportWaveRTNovaMix
(
    _Out_       PUNKNOWN *,
    _In_        REFCLSID,
    _In_opt_    PUNKNOWN,
    _In_        POOL_FLAGS,
    _In_        PUNKNOWN,
    _In_opt_    PVOID,
    _In_        PENDPOINT_MINIPAIR
);

NTSTATUS
CreateMiniportTopologyNovaMix
(
    _Out_       PUNKNOWN *,
    _In_        REFCLSID,
    _In_opt_    PUNKNOWN,
    _In_        POOL_FLAGS,
    _In_        PUNKNOWN,
    _In_opt_    PVOID,
    _In_        PENDPOINT_MINIPAIR
);

/*********************************************************************
* Topology/Wave bridge connection for the render endpoints           *
*                                                                    *
*              +------+                +------+                      *
*              | Wave |                | Topo |                      *
*              |      |                |      |                      *
* System   --->|0    1|--------------->|0    1|---> Endpoint         *
*              |      |                |      |                      *
*              +------+                +------+                      *
*********************************************************************/
static
PHYSICALCONNECTIONTABLE RenderTopologyPhysicalConnections[] =
{
    {
        KSPIN_TOPO_WAVEOUT_SOURCE,  // TopologyIn
        KSPIN_WAVE_RENDER_SOURCE,   // WaveOut
        CONNECTIONTYPE_WAVE_OUTPUT
    }
};

// "NovaMix Virtual Playback": null sink, captured by WASAPI loopback.
static
ENDPOINT_MINIPAIR PlaybackMiniports =
{
    eNovaMixPlaybackDevice,
    L"TopologyPlayback",                                    // KSNAME_TopologyPlayback in the INF
    NULL,                                                   // optional template name
    CreateMiniportTopologyNovaMix,
    &PlaybackTopoMiniportFilterDescriptor,
    0, NULL,                                                // Interface properties
    L"WavePlayback",                                        // KSNAME_WavePlayback in the INF
    NULL,                                                   // optional template name
    CreateMiniportWaveRTNovaMix,
    &PlaybackWaveMiniportFilterDescriptor,
    0,                                                      // Interface properties
    NULL,
    NOVAMIX_DEVICE_MAX_CHANNELS,
    PlaybackPinDeviceFormatsAndModes,
    SIZEOF_ARRAY(PlaybackPinDeviceFormatsAndModes),
    RenderTopologyPhysicalConnections,
    SIZEOF_ARRAY(RenderTopologyPhysicalConnections),
    ENDPOINT_NO_FLAGS,
};

// "NovaMix Mic Feed": what is rendered here goes to the loopback ring.
static
ENDPOINT_MINIPAIR MicFeedMiniports =
{
    eNovaMixMicFeedDevice,
    L"TopologyMicFeed",                                     // KSNAME_TopologyMicFeed in the INF
    NULL,                                                   // optional template name
    CreateMiniportTopologyNovaMix,
    &MicFeedTopoMiniportFilterDescriptor,
    0, NULL,                                                // Interface properties
    L"WaveMicFeed",                                         // KSNAME_WaveMicFeed in the INF
    NULL,                                                   // optional template name
    CreateMiniportWaveRTNovaMix,
    &MicFeedWaveMiniportFilterDescriptor,
    0,                                                      // Interface properties
    NULL,
    NOVAMIX_DEVICE_MAX_CHANNELS,
    MicFeedPinDeviceFormatsAndModes,
    SIZEOF_ARRAY(MicFeedPinDeviceFormatsAndModes),
    RenderTopologyPhysicalConnections,
    SIZEOF_ARRAY(RenderTopologyPhysicalConnections),
    ENDPOINT_NO_FLAGS,
};

/*********************************************************************
* Topology/Wave bridge connection for the capture endpoint           *
*                                                                    *
*              +------+    +------+                                  *
*              | Topo |    | Wave |                                  *
*              |      |    |      |                                  *
*  Mic in  --->|0    1|===>|0    1|---> Capture Host Pin             *
*              |      |    |      |                                  *
*              +------+    +------+                                  *
*********************************************************************/
static
PHYSICALCONNECTIONTABLE MicTopologyPhysicalConnections[] =
{
    {
        KSPIN_TOPO_BRIDGE,          // TopologyOut
        KSPIN_WAVE_BRIDGE,          // WaveIn
        CONNECTIONTYPE_TOPOLOGY_OUTPUT
    }
};

// "NovaMix Virtual Mic": reads the loopback ring.
static
ENDPOINT_MINIPAIR MicMiniports =
{
    eNovaMixMicDevice,
    L"TopologyMic",                                         // KSNAME_TopologyMic in the INF
    NULL,                                                   // optional template name
    CreateMiniportTopologyNovaMix,
    &MicTopoMiniportFilterDescriptor,
    0, NULL,                                                // Interface properties
    L"WaveMic",                                             // KSNAME_WaveMic in the INF
    NULL,                                                   // optional template name
    CreateMiniportWaveRTNovaMix,
    &MicWaveMiniportFilterDescriptor,
    0,                                                      // Interface properties
    NULL,
    NOVAMIX_DEVICE_MAX_CHANNELS,
    MicPinDeviceFormatsAndModes,
    SIZEOF_ARRAY(MicPinDeviceFormatsAndModes),
    MicTopologyPhysicalConnections,
    SIZEOF_ARRAY(MicTopologyPhysicalConnections),
    ENDPOINT_NO_FLAGS,
};

//=============================================================================
static
PENDPOINT_MINIPAIR  g_Endpoints[] =
{
    &PlaybackMiniports,
    &MicFeedMiniports,
    &MicMiniports,
};

#define g_cEndpoints    (SIZEOF_ARRAY(g_Endpoints))

//=============================================================================
//
// Total miniports = # endpoints * 2 (topology + wave).
//
#define g_MaxMiniports  (g_cEndpoints * 2)

#endif // _NOVAMIX_MINIPAIRS_H_
