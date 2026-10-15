# Adaptador macOS (Core Audio) — fase 6

> **Status:** planejado.

## Estratégia

* Endpoints virtuais via **Audio Server Plug-in** (HAL plug-in), partindo do exemplo oficial da Apple
  ["Creating an Audio Server Driver Plug-in"](https://developer.apple.com/documentation/coreaudio/creating-an-audio-server-driver-plug-in).
  O plug-in roda no processo `coreaudiod` (modo usuário), sem extensão de kernel.
* Mesmo contrato de nomes do Windows: `NovaMix Virtual Playback`, `NovaMix Mic Feed`,
  `NovaMix Virtual Mic` (loopback interno *Mic Feed → Virtual Mic*; o *Playback* expõe também uma entrada
  "monitor" já que o Core Audio não tem loopback de saída nativo).
* Dispositivos físicos pelo backend Core Audio do CPAL (já suportado).
* `DeviceWatcher` via `AudioObjectAddPropertyListener` (lista de dispositivos e padrão).

## Distribuição

* Plug-in assinado com Developer ID e **notarizado**; instalação em
  `/Library/Audio/Plug-Ins/HAL/` (requer administrador) e reinício do `coreaudiod`.
* Permissão de microfone (TCC) para o app.
* Revalidar os requisitos a cada versão do macOS suportada.

## Critérios para suporte

* macOS atual e anterior, Apple Silicon e Intel.
* `novamix-cli devices` reconhece os endpoints; soak test de 30 min sem underruns.
