
/*++

Copyright (c) Microsoft Corporation All Rights Reserved

Derived from the Windows-driver-samples SimpleAudioSample (SysVAD family),
MS-PL. Modified for the NovaMix virtual audio driver.

Module Name:

    minitopo.h

Abstract:

    Declaration of topology miniport.
--*/

#ifndef _NOVAMIX_MINTOPO_H_
#define _NOVAMIX_MINTOPO_H_

#include "basetopo.h"

//=============================================================================
// Classes
//=============================================================================

///////////////////////////////////////////////////////////////////////////////
// CMiniportTopology 
//   

class CMiniportTopology : 
    public CMiniportTopologyNovaMix,
    public IMiniportTopology,
    public CUnknown
{
  private:
    eDeviceType             m_DeviceType;

    union {
        PVOID               m_DeviceContext;
    };

public:
    DECLARE_STD_UNKNOWN();
    CMiniportTopology
    (
        _In_opt_    PUNKNOWN                UnknownOuter,
        _In_        PCFILTER_DESCRIPTOR    *FilterDesc,
        _In_        USHORT                  DeviceMaxChannels,
        _In_        eDeviceType             DeviceType, 
        _In_opt_    PVOID                   DeviceContext
    )
    : CUnknown(UnknownOuter),
      CMiniportTopologyNovaMix(FilterDesc, DeviceMaxChannels),
      m_DeviceType(DeviceType),
      m_DeviceContext(DeviceContext)
    {
    }

    ~CMiniportTopology();

    IMP_IMiniportTopology;

    NTSTATUS PropertyHandlerJackDescription
    (
        _In_        PPCPROPERTY_REQUEST                         PropertyRequest,
        _In_        ULONG                                       cJackDescriptions,
        _In_reads_(cJackDescriptions) PKSJACK_DESCRIPTION       *JackDescriptions
    );

    NTSTATUS PropertyHandlerJackDescription2
    ( 
        _In_        PPCPROPERTY_REQUEST                         PropertyRequest,
        _In_        ULONG                                       cJackDescriptions,
        _In_reads_(cJackDescriptions) PKSJACK_DESCRIPTION       *JackDescriptions,
        _In_        DWORD                                       JackCapabilities
    );
    
    PVOID GetDeviceContext() { return m_DeviceContext;  }

    eDeviceType GetDeviceType() { return m_DeviceType; }
};

typedef CMiniportTopology *PCMiniportTopology;

#endif // _NOVAMIX_MINTOPO_H_
