# NovaMix Virtual Audio Driver (Windows)

> **Status (2026-10-14):** implementado e compilando (x64, `/W4 /WX`, Code Analysis limpa no código
> novo, InfVerif `/w` e `/h` válidos). O pacote é gerado em `package/` **assinado com certificado de teste**.
> **Ainda não validado em VM**: instalação/remoção, detecção pelo app e o teste de 30 min precisam ser
> executados numa VM com *test signing* (ver [Validação](#validação)). Distribuição para usuários exige
> assinatura de atestação ([D6](../../docs/decisions.md)).

## Objetivo

O **mínimo** de driver para expor endpoints de áudio virtuais. Toda a lógica (mixagem, DSP, roteamento,
compensação de deriva) fica no modo usuário, no NovaMix. Sem IOCTLs nem interface privada: o app usa só
WASAPI.

## Endpoints (contrato com o app)

Os nomes são o contrato com `crates/device-windows/src/virtual_endpoints.rs` (`NOVAMIX_PLAYBACK`,
`NOVAMIX_MIC_FEED`, `NOVAMIX_MIC`). Eles vêm de `[Strings]` no INF (`Endpoint.*`, gravados em
`MediaCategories` para os GUIDs de nome dos pinos em `src/endpoints.h`). O Windows mostra
`"<nome> (NovaMix Virtual Audio Device)"`; o app compara por prefixo.

| Nome | Tipo / form factor | Formatos | Função |
|---|---|---|---|
| `NovaMix Virtual Playback` | Render, Speakers | 44,1/48/96 kHz | Null sink: os quadros são descartados; o NovaMix captura por **loopback WASAPI** (feito pelo audio engine). |
| `NovaMix Mic Feed` | Render, Line | 48 kHz | O NovaMix escreve o bus de voz aqui; os quadros vão para o anel interno. |
| `NovaMix Virtual Mic` | Capture, Microphone | 48 kHz | Apps gravam daqui; lê o anel interno (silêncio quando não há dados). |

Todos: PCM 16/24/32 bits e float 32 bits, 1–2 canais. Formato padrão (primeira entrada das tabelas,
`PKEY_AudioEngine_OEMFormat` no INF e `KSPROPERTY_PIN_PROPOSEDATAFORMAT2`): **48 kHz, 2 canais, float 32**.
As topologias não têm nós de volume/mute, então o Windows aplica o volume do endpoint em software e os
controles de volume de cada endpoint funcionam normalmente.

```text
 Apps ──► [NovaMix Virtual Playback] ··(loopback WASAPI)··► NovaMix
 NovaMix ──► [NovaMix Mic Feed] ══(anel interno, ~20 ms)══► [NovaMix Virtual Mic] ──► Apps
```

### Anel Mic Feed → Virtual Mic (`src/loopback.*`)

* Um único anel global (200 ms, float estéreo, 48 kHz) com spinlock; escrito no DPC/consulta de posição
  do stream do Mic Feed e lido no do Virtual Mic. Converte canais (mono ↔ estéreo) e tipo de amostra.
* Os dois lados usam o mesmo relógio (timer do sistema), então não há deriva entre eles; o anel só absorve
  o jitter. O leitor espera um colchão de `MicLoopbackLatencyMs` (padrão 20 ms, 5–100, em
  `HKLM\SYSTEM\CurrentControlSet\Services\NovaMixVirtualAudio\Parameters`) antes de entregar dados; em
  falta de dados entrega silêncio e espera um novo colchão; acima de latência + 80 ms descarta o excesso.
* Mic Feed e Virtual Mic só oferecem 48 kHz para não precisar de conversão de taxa no kernel (o NovaMix já
  reamostra no modo usuário).
* Conteúdo com `CopyProtect` (DRM) tocado no Mic Feed não é repassado ao Virtual Mic. O INF não declara
  nível DRM.

## Origem do código

Derivado do exemplo oficial **SimpleAudioSample** do repositório
[Windows-driver-samples](https://github.com/microsoft/Windows-driver-samples) (commit `2dc3fd3`,
2026-09-25). Ele é a versão reduzida do SysVAD mantida pela Microsoft (o mesmo código de PortCls/WaveRT da
variante TabletAudioSample, já sem Bluetooth/USB sideband, keyword spotter, offload e endpoints extras).
Partir dele em vez de reduzir o TabletAudioSample deu o mesmo resultado com muito menos código removido à
mão. Mudanças: três endpoints, tabelas de formato próprias, interseção de formatos com float, anel de
loopback no lugar do gerador de tom e da gravação em arquivo, topologias sem nós, jack sem detecção de
presença. Os arquivos derivados mantêm o aviso de copyright da Microsoft; licença em `LICENSE-MS-PL.txt`.

## Estrutura

```text
platform/windows-driver/
  NovaMixVirtualAudio.vcxproj   projeto WDK (KMDF 1.15, alvo Windows 10 2004+ / 11, x64)
  NovaMixVirtualAudio.inx       INF (stampinf gera o .inf; dispositivo ROOT\NovaMixVirtualAudio)
  src/                          adapter, common, minwavert*, mintopo, basetopo, loopback, tabelas
  scripts/build-driver.ps1      build + assinatura + cópia para package/
  scripts/novamix-driver.ps1    instala/remove (usado pelo instalador; ver "Instalação no produto")
  tests/install-driver.ps1      instala numa VM (chama novamix-driver.ps1 -Action Install)
  tests/uninstall-driver.ps1    remove numa VM (novamix-driver.ps1 -Action Uninstall + certificado)
  tests/check-endpoints.ps1     confere os endpoints no Windows e no novamix-cli (+ teste longo)
  package/                      pacote gerado (sys, inf, cat, scripts, certificado de teste)
  build/                        saída do MSBuild (ignorada)
```

## Build

Pré-requisitos (instalados nesta máquina em 2026-10-14): Visual Studio 2022 com os componentes
*Windows Driver Kit* e *MSVC Spectre-mitigated libs*, Windows SDK 10.0.26100 e WDK 10.0.26100
(`winget install --source winget --exact --id Microsoft.WindowsWDK.10.0.26100`).

```powershell
powershell -ExecutionPolicy Bypass -File platform\windows-driver\scripts\build-driver.ps1
```

Sem `NOVAMIX_DRIVER_SIGN_CERT`, o script cria (uma vez) o certificado autoassinado
`CN=NovaMix Test Driver Signing` em `CurrentUser\My`, assina `.sys` e `.cat`, exporta
`package\NovaMixTestCert.cer` e marca o pacote com `TEST-SIGNED.txt`. O
`packaging/windows/build-release.ps1` **não** inclui pacotes de teste no instalador (só com
`-IncludeTestSignedDriver`).

## Validação

Numa VM Windows 10 2004+/11 x64 (Hyper-V: Secure Boot desligado na VM):

1. `bcdedit /set testsigning on` e reiniciar.
2. Copiar `package\` para a VM e, como administrador: `.\install-driver.ps1` — deve listar os três endpoints
   e manter os dispositivos padrão anteriores.
3. Copiar `novamix-cli.exe` e rodar `.\check-endpoints.ps1 -Cli .\novamix-cli.exe` — deve mostrar
   `NovaMix driver installed: true`; com `-SoakSeconds 1800` roda `novamix-cli run --seconds 1800` (sem
   underruns esperados).
4. Teste do cabo: tocar um tom no *NovaMix Mic Feed* (`novamix-cli tone`) e gravar do *NovaMix Virtual Mic*
   (Gravador de Som), também com formatos diferentes nos dois lados (mono/16 bits etc.).
5. Opcional: Driver Verifier (`verifier /standard /driver NovaMixVirtualAudio.sys`) durante os passos 2–4.
6. `.\uninstall-driver.ps1 -RemoveCertificate` — os endpoints devem sumir.

## Instalação no produto

O instalador (`packaging/windows/novamix.iss`) copia o pacote para `{app}\driver` e, se o componente
*driver* foi escolhido, explica por que vai pedir administrador e roda via UAC:

```text
powershell -NoProfile -ExecutionPolicy Bypass -File {app}\driver\novamix-driver.ps1 -Action Install
```

`novamix-driver.ps1 -Action Install`:

1. salva os dispositivos padrão de reprodução/gravação (papéis console, multimídia e comunicações) em
   `%ProgramData%\NovaMix\driver-defaults.json`. Papéis que já apontam para um endpoint NovaMix (reinstalação)
   mantêm o valor salvo antes;
2. só em pacotes `TEST-SIGNED`: confia em `NovaMixTestCert.cer` (VMs de teste);
3. cria o dispositivo `ROOT\NovaMixVirtualAudio` via SetupAPI, se ainda não existir. O `pnputil` sozinho
   não cria dispositivos enumerados pela raiz;
4. roda `pnputil /add-driver NovaMixVirtualAudio.inf /install` e espera os três endpoints (60 s);
5. em falha remove o dispositivo e o pacote, restaura os padrões e sai com código 1. O instalador avisa que o
   NovaMix segue funcionando com cabos de terceiros;
6. em sucesso, se o Windows tornou um endpoint NovaMix o padrão, volta o dispositivo salvo.

Na desinstalação, o desinstalador pergunta se o driver deve ser removido (padrão: sim; UAC) e roda
`-Action Uninstall`. Antes de remover, o script anota quais papéis apontam para endpoints NovaMix. Depois
remove os dispositivos e todos os pacotes `NovaMixVirtualAudio.inf` do driver store e devolve só esses
papéis aos dispositivos salvos, se eles ainda existirem. Papéis que você mudou para outro dispositivo real
ficam como estão.

Logs de cada execução ficam em `%ProgramData%\NovaMix\logs\driver-install.log` e `driver-uninstall.log`.
`-Action ShowDefaults` só lê: mostra os padrões atuais, os endpoints NovaMix e o que foi salvo. A troca de
dispositivo padrão usa `IPolicyConfig`, a interface não documentada (estável desde o Windows 7) que o
painel de Som também usa.

## Riscos e mitigação

| Risco | Mitigação |
|---|---|
| Driver instável derruba o sistema | Driver mínimo, sem lógica de áudio; Code Analysis; Driver Verifier; testes em VM |
| Assinatura/aprovação | Assinatura de atestação no Hardware Dev Center; `NOVAMIX_DRIVER_SIGN_CERT` + `signtool` no pipeline |
| Deriva entre timer do driver e dispositivos reais | Compensada pelas pontes do NovaMix (controle PI) |
| Jitter dos DPCs no anel Mic Feed → Virtual Mic | Colchão de 20 ms ajustável (`MicLoopbackLatencyMs`) |
| Apps escolhem o *Mic Feed* por engano | Nome deixa claro o papel; não há como ocultar um endpoint pelo INF |
