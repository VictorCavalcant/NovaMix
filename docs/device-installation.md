# Dispositivos virtuais: instalação e uso

O NovaMix precisa de dois tipos de endpoint virtual para cumprir o roteamento do plano:

| Endpoint | Direção | Para que serve |
|---|---|---|
| **Reprodução virtual** | saída (render) que os apps usam | O áudio dos apps entra no canal *Desktop/Virtual In* |
| **Microfone virtual** | entrada (capture) que os apps usam | Os apps recebem o bus de voz processado |

CPAL/WASAPI acessam dispositivos existentes e fazem captura de *loopback*, mas **não criam** endpoints.
Para isso é preciso um driver (ver [`platform/windows-driver`](../platform/windows-driver/README.md)).

## Ordem de detecção

1. **Driver NovaMix** — endpoints com os nomes contratados:
   * `NovaMix Virtual Playback` (render): apps tocam nele; o NovaMix captura por loopback.
   * `NovaMix Mic Feed` (render) → `NovaMix Virtual Mic` (capture): o NovaMix escreve no *feed*; os apps
     gravam do *Virtual Mic*.
2. **Fallback de desenvolvimento** — cabos virtuais já instalados, pareados pelo nome
   (`X Input` ↔ `X Output`, ou nomes idênticos no Virtual Audio Cable):
   * 1º cabo (preferência pelo VB-Audio Virtual Cable) → microfone virtual;
   * 2º cabo → reprodução virtual (o NovaMix lê o lado *Output* do cabo, que entrega áudio contínuo).

O status aparece na barra inferior, no assistente e em *Diagnóstico → Endpoints virtuais*:
"Driver virtual NovaMix instalado", "Endpoints virtuais de desenvolvimento" ou "Sem endpoints virtuais".

## Configuração de desenvolvimento recomendada

1. Instale o [VB-Audio Virtual Cable](https://vb-audio.com/Cable/) (e, opcionalmente, o Hi-Fi Cable ou
   o pacote A+B para ter a reprodução virtual).
2. Abra o NovaMix; o assistente mostra:
   * "Nos seus apps, selecione como microfone: **CABLE Output (VB-Audio Virtual Cable)**";
   * "Para mixar o som de um app, envie a saída dele para: **Hi-Fi Cable Input**".
3. No Discord/Zoom/OBS, escolha o microfone indicado. No Windows (*Configurações → Som → Mixer de
   volume*), envie apps específicos para a reprodução virtual.

Sem o segundo cabo, o canal Desktop pode usar **Loopback da saída padrão**. Atenção: se a monitoração
também for a saída padrão, isso forma um ciclo; o NovaMix detecta, bloqueia o envio e oferece
**Corrigir rota**.

## Driver NovaMix (produção)

O driver é um projeto separado (C/C++ com WDK) derivado do exemplo SYSVAD. Instalação planejada:

```text
pnputil /add-driver NovaMixVirtualAudio.inf /install   (requer administrador, pacote assinado)
```

Remoção:

```text
pnputil /delete-driver oemNN.inf /uninstall
```

O instalador (`packaging/windows`) explicará as permissões, fará backup do dispositivo padrão anterior e
permitirá reverter. Enquanto o driver não estiver disponível, o NovaMix funciona com o fallback acima.

## Segurança

* Privilégios administrativos só são necessários para instalar/remover o driver.
* O NovaMix nunca baixa nem executa componentes de áudio.
* O driver não terá interface própria com o modo usuário: o áudio passa apenas pelos endpoints WASAPI
  padrão (superfície de ataque mínima).
