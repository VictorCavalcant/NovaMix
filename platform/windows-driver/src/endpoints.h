/*++

Copyright (c) Microsoft Corporation All Rights Reserved

Derived from the Windows-driver-samples SimpleAudioSample (SysVAD family),
MS-PL. Modified for the NovaMix virtual audio driver.

Module Name:

    endpoints.h

Abstract:

    Node and pin numbers and other common definitions for the three NovaMix
    endpoints. The endpoint names are a contract with the NovaMix app
    (crates/device-windows/src/virtual_endpoints.rs) and are set in the INF
    under MediaCategories for the GUIDs below.
--*/

#ifndef _NOVAMIX_ENDPOINTS_H_
#define _NOVAMIX_ENDPOINTS_H_

// Pin name GUIDs. The INF maps each one to a friendly endpoint name:
//   NAME_NOVAMIX_PLAYBACK -> "NovaMix Virtual Playback"
//   NAME_NOVAMIX_MIC_FEED -> "NovaMix Mic Feed"
//   NAME_NOVAMIX_MIC      -> "NovaMix Virtual Mic"

// {2B829421-11A6-4292-9532-4E10F4E2D1F0}
#define STATIC_NAME_NOVAMIX_PLAYBACK\
    0x2b829421, 0x11a6, 0x4292, 0x95, 0x32, 0x4e, 0x10, 0xf4, 0xe2, 0xd1, 0xf0
DEFINE_GUIDSTRUCT("2B829421-11A6-4292-9532-4E10F4E2D1F0", NAME_NOVAMIX_PLAYBACK);
#define NAME_NOVAMIX_PLAYBACK DEFINE_GUIDNAMED(NAME_NOVAMIX_PLAYBACK)

// {B7803720-01E6-49A4-B3D2-85E23AB19EBC}
#define STATIC_NAME_NOVAMIX_MIC_FEED\
    0xb7803720, 0x01e6, 0x49a4, 0xb3, 0xd2, 0x85, 0xe2, 0x3a, 0xb1, 0x9e, 0xbc
DEFINE_GUIDSTRUCT("B7803720-01E6-49A4-B3D2-85E23AB19EBC", NAME_NOVAMIX_MIC_FEED);
#define NAME_NOVAMIX_MIC_FEED DEFINE_GUIDNAMED(NAME_NOVAMIX_MIC_FEED)

// {0A146EEA-2D6E-40AA-B765-3A931BC1F122}
#define STATIC_NAME_NOVAMIX_MIC\
    0x0a146eea, 0x2d6e, 0x40aa, 0xb7, 0x65, 0x3a, 0x93, 0x1b, 0xc1, 0xf1, 0x22
DEFINE_GUIDSTRUCT("0A146EEA-2D6E-40AA-B765-3A931BC1F122", NAME_NOVAMIX_MIC);
#define NAME_NOVAMIX_MIC DEFINE_GUIDNAMED(NAME_NOVAMIX_MIC)

//----------------------------------------------------
// Render endpoints (Virtual Playback, Mic Feed).
//----------------------------------------------------

// Wave pins - no mix, no offload, no hardware loopback (the audio engine
// provides WASAPI loopback in software).
enum
{
    KSPIN_WAVE_RENDER_SINK_SYSTEM = 0,
    KSPIN_WAVE_RENDER_SOURCE
};

// Topology pins.
enum
{
    KSPIN_TOPO_WAVEOUT_SOURCE = 0,
    KSPIN_TOPO_LINEOUT_DEST,
};

//----------------------------------------------------
// Capture endpoint (Virtual Mic).
//----------------------------------------------------

// Wave pins
enum
{
    KSPIN_WAVE_BRIDGE = 0,
    KSPIN_WAVEIN_HOST,
};

// Wave topology nodes.
enum
{
    KSNODE_WAVE_ADC = 0
};

// Topology pins.
enum
{
    KSPIN_TOPO_MIC_ELEMENTS,
    KSPIN_TOPO_BRIDGE
};

// The topology filters have no volume/mute nodes on purpose: without them
// Windows applies endpoint volume in software, so the volume slider of each
// NovaMix endpoint keeps working.

// data format attribute range definitions.
static
KSATTRIBUTE PinDataRangeSignalProcessingModeAttribute =
{
    sizeof(KSATTRIBUTE),
    0,
    STATICGUIDOF(KSATTRIBUTEID_AUDIOSIGNALPROCESSING_MODE),
};

static
PKSATTRIBUTE PinDataRangeAttributes[] =
{
    &PinDataRangeSignalProcessingModeAttribute,
};

static
KSATTRIBUTE_LIST PinDataRangeAttributeList =
{
    ARRAYSIZE(PinDataRangeAttributes),
    PinDataRangeAttributes,
};

#endif // _NOVAMIX_ENDPOINTS_H_
