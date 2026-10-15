/*++

Copyright (c) Microsoft Corporation All Rights Reserved

Derived from the Windows-driver-samples SimpleAudioSample (SysVAD family),
MS-PL. Modified for the NovaMix virtual audio driver.

Module Name:

    toptables.h

Abstract:

    Topology miniport tables for the three NovaMix endpoints. Each topology
    is a straight connection between the wave bridge pin and the endpoint
    pin; the endpoint pin carries the name GUID that the INF maps to the
    friendly endpoint name.
--*/

#ifndef _NOVAMIX_TOPTABLES_H_
#define _NOVAMIX_TOPTABLES_H_

//=============================================================================
static
KSDATARANGE TopoPinDataRangesBridge[] =
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
PKSDATARANGE TopoPinDataRangePointersBridge[] =
{
    &TopoPinDataRangesBridge[0]
};

//=============================================================================
static
PCPROPERTY_ITEM PropertiesTopoFilter[] =
{
    {
        &KSPROPSETID_Jack,
        KSPROPERTY_JACK_DESCRIPTION,
        KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_BASICSUPPORT,
        PropertyHandler_TopoFilter
    },
    {
        &KSPROPSETID_Jack,
        KSPROPERTY_JACK_DESCRIPTION2,
        KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_BASICSUPPORT,
        PropertyHandler_TopoFilter
    }
};

DEFINE_PCAUTOMATION_TABLE_PROP(AutomationTopoFilter, PropertiesTopoFilter);

//=============================================================================
// Render topologies (Virtual Playback, Mic Feed)
//=============================================================================
#define NOVAMIX_RENDER_TOPO_PINS(EndpointCategory, EndpointName)                \
    {                                                                           \
        /* KSPIN_TOPO_WAVEOUT_SOURCE */                                         \
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
                SIZEOF_ARRAY(TopoPinDataRangePointersBridge),                   \
                TopoPinDataRangePointersBridge,                                 \
                KSPIN_DATAFLOW_IN,                                              \
                KSPIN_COMMUNICATION_NONE,                                       \
                &KSCATEGORY_AUDIO,                                              \
                NULL,                                                           \
                0                                                               \
            }                                                                   \
        },                                                                      \
        /* KSPIN_TOPO_LINEOUT_DEST */                                           \
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
                SIZEOF_ARRAY(TopoPinDataRangePointersBridge),                   \
                TopoPinDataRangePointersBridge,                                 \
                KSPIN_DATAFLOW_OUT,                                             \
                KSPIN_COMMUNICATION_NONE,                                       \
                (EndpointCategory),                                             \
                (EndpointName),                                                 \
                0                                                               \
            }                                                                   \
        }                                                                       \
    }

static
PCPIN_DESCRIPTOR PlaybackTopoMiniportPins[] =
    NOVAMIX_RENDER_TOPO_PINS(&KSNODETYPE_SPEAKER, &NAME_NOVAMIX_PLAYBACK);

static
PCPIN_DESCRIPTOR MicFeedTopoMiniportPins[] =
    NOVAMIX_RENDER_TOPO_PINS(&KSNODETYPE_LINE_CONNECTOR, &NAME_NOVAMIX_MIC_FEED);

static
PCCONNECTION_DESCRIPTOR RenderTopoMiniportConnections[] =
{
    //  FromNode,       FromPin,                    ToNode,         ToPin
    {   PCFILTER_NODE,  KSPIN_TOPO_WAVEOUT_SOURCE,  PCFILTER_NODE,  KSPIN_TOPO_LINEOUT_DEST }
};

static
PCFILTER_DESCRIPTOR PlaybackTopoMiniportFilterDescriptor =
{
    0,                                              // Version
    &AutomationTopoFilter,                          // AutomationTable
    sizeof(PCPIN_DESCRIPTOR),                       // PinSize
    SIZEOF_ARRAY(PlaybackTopoMiniportPins),         // PinCount
    PlaybackTopoMiniportPins,                       // Pins
    sizeof(PCNODE_DESCRIPTOR),                      // NodeSize
    0,                                              // NodeCount
    NULL,                                           // Nodes
    SIZEOF_ARRAY(RenderTopoMiniportConnections),    // ConnectionCount
    RenderTopoMiniportConnections,                  // Connections
    0,                                              // CategoryCount
    NULL                                            // Categories
};

static
PCFILTER_DESCRIPTOR MicFeedTopoMiniportFilterDescriptor =
{
    0,                                              // Version
    &AutomationTopoFilter,                          // AutomationTable
    sizeof(PCPIN_DESCRIPTOR),                       // PinSize
    SIZEOF_ARRAY(MicFeedTopoMiniportPins),          // PinCount
    MicFeedTopoMiniportPins,                        // Pins
    sizeof(PCNODE_DESCRIPTOR),                      // NodeSize
    0,                                              // NodeCount
    NULL,                                           // Nodes
    SIZEOF_ARRAY(RenderTopoMiniportConnections),    // ConnectionCount
    RenderTopoMiniportConnections,                  // Connections
    0,                                              // CategoryCount
    NULL                                            // Categories
};

//=============================================================================
// Capture topology (Virtual Mic)
//=============================================================================
static
PCPIN_DESCRIPTOR MicTopoMiniportPins[] =
{
    // KSPIN_TOPO_MIC_ELEMENTS
    {
        0,
        0,
        0,                                              // InstanceCount
        NULL,                                           // AutomationTable
        {                                               // KsPinDescriptor
            0,                                          // InterfacesCount
            NULL,                                       // Interfaces
            0,                                          // MediumsCount
            NULL,                                       // Mediums
            SIZEOF_ARRAY(TopoPinDataRangePointersBridge),   // DataRangesCount
            TopoPinDataRangePointersBridge,             // DataRanges
            KSPIN_DATAFLOW_IN,                          // DataFlow
            KSPIN_COMMUNICATION_NONE,                   // Communication
            &KSNODETYPE_MICROPHONE,                     // Category
            &NAME_NOVAMIX_MIC,                          // Name
            0                                           // Reserved
        }
    },
    // KSPIN_TOPO_BRIDGE
    {
        0,
        0,
        0,                                              // InstanceCount
        NULL,                                           // AutomationTable
        {                                               // KsPinDescriptor
            0,                                          // InterfacesCount
            NULL,                                       // Interfaces
            0,                                          // MediumsCount
            NULL,                                       // Mediums
            SIZEOF_ARRAY(TopoPinDataRangePointersBridge),   // DataRangesCount
            TopoPinDataRangePointersBridge,             // DataRanges
            KSPIN_DATAFLOW_OUT,                         // DataFlow
            KSPIN_COMMUNICATION_NONE,                   // Communication
            &KSCATEGORY_AUDIO,                          // Category
            NULL,                                       // Name
            0                                           // Reserved
        }
    }
};

static
PCCONNECTION_DESCRIPTOR MicTopoMiniportConnections[] =
{
    //  FromNode,       FromPin,                    ToNode,         ToPin
    {   PCFILTER_NODE,  KSPIN_TOPO_MIC_ELEMENTS,    PCFILTER_NODE,  KSPIN_TOPO_BRIDGE }
};

static
PCFILTER_DESCRIPTOR MicTopoMiniportFilterDescriptor =
{
    0,                                              // Version
    &AutomationTopoFilter,                          // AutomationTable
    sizeof(PCPIN_DESCRIPTOR),                       // PinSize
    SIZEOF_ARRAY(MicTopoMiniportPins),              // PinCount
    MicTopoMiniportPins,                            // Pins
    sizeof(PCNODE_DESCRIPTOR),                      // NodeSize
    0,                                              // NodeCount
    NULL,                                           // Nodes
    SIZEOF_ARRAY(MicTopoMiniportConnections),       // ConnectionCount
    MicTopoMiniportConnections,                     // Connections
    0,                                              // CategoryCount
    NULL                                            // Categories
};

#endif // _NOVAMIX_TOPTABLES_H_
