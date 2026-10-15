# Registro de decisões

Decisões exigidas pela seção 18 do planejamento, mais as decisões de engenharia tomadas na implementação.
Cada item registra o contexto, a decisão e como revisá-la.

## D1 — Versões do Windows suportadas

**Decisão:** Windows 10 22H2 e Windows 11 (23H2+), x64.
**Motivo:** WASAPI compartilhado com `AUTOCONVERTPCM`, `IMMNotificationClient` e temporizadores de alta
resolução estão disponíveis e estáveis nessas versões; o toolchain Rust MSVC é o alvo principal.
**Revisar:** ao planejar ARM64 (cpal e egui suportam; o driver precisará de build ARM64 separado).

## D2 — Driver próprio desde o primeiro protótipo?

**Decisão:** não. O MVP começa com **endpoints virtuais existentes** em desenvolvimento (VB-Audio Virtual
Cable, Hi-Fi Cable, VoiceMeeter, Virtual Audio Cable), detectados por nome em
`novamix-device-windows::virtual_endpoints`. O driver NovaMix segue em paralelo (projeto em
`platform/windows-driver`) com um contrato de nomes de endpoint fixo, de modo que o app passe a usá-lo sem
mudanças.
**Motivo:** reduz o risco principal (driver) sem bloquear motor, DSP, UI e testes.
**Estado (2026-10-14):** driver implementado em `platform/windows-driver` (PortCls/WaveRT, derivado do
SimpleAudioSample, a versão reduzida do SysVAD), com os três endpoints do contrato e um anel interno de 20 ms
Mic Feed → Virtual Mic. Compila sem avisos, passa na Code Analysis e no InfVerif, e gera um pacote assinado com
certificado de teste. Mic Feed/Virtual Mic ficam fixos em 48 kHz (sem conversão de taxa no kernel). **Ainda não
validado em VM**; os cabos de terceiros continuam como fallback até lá.
**Revisar:** quando o driver passar nos testes de instalação/remoção e no teste de 30 min em VM
(`platform/windows-driver/README.md`, seção Validação).

## D3 — Formatos, taxas e buffers testados oficialmente

**Decisão:** motor em `f32` interno; dispositivos em qualquer formato que o CPAL entregue (f32, i16, i24,
i32, u8, f64), convertidos fora do callback do motor. Taxas oficialmente testadas: **48 kHz** (padrão),
**44,1 kHz**; 96 kHz suportado quando o dispositivo mestre oferece. Buffers: 128/256/512 frames pelos
modos de latência; 64 e 1024 manualmente. No WASAPI compartilhado o período real é definido pelo Windows
(tipicamente 10 ms) e o tamanho pedido afeta apenas a latência do buffer.
**Atualização (2026-10-14):** backend WASAPI próprio como sistema selecionável — `wasapi-lowlatency`
(IAudioClient3) e `wasapi-exclusive`; nele o tamanho de buffer do modo de latência vira o período do
dispositivo (arredondado ao período fundamental, dentro da faixa do driver). Fica em `novamix-device-api`
(feature `wasapi-lowlatency`) e não em `novamix-device-windows` porque `create_backend` precisa vê-lo e
`device-windows` já depende de `device-api`. O CPAL continua o padrão até o backend próprio acumular horas
de uso; números em [audio-performance.md](audio-performance.md).

## D4 — GUI e crates de áudio

**Decisão:** `egui`/`eframe 0.36` (renderizador glow, accesskit habilitado para leitores de tela),
`cpal 0.18` (WASAPI; ASIO opcional pela feature `asio`), `rtrb 0.4` (SPSC tempo real), `realfft 3.5`.
Versões fixadas no `Cargo.toml` do workspace e no `Cargo.lock`; toolchain fixada em `rust-toolchain.toml`.
**Motivo:** egui permite medidores e controles customizados com pouco código e boa performance; o CPAL
cobre WASAPI/ASIO e Linux/macOS para fases futuras.

## D5 — Redução de ruído: algoritmos e licenças

**Decisão:** dois modos, ambos locais:
* **Espectral** (implementação própria): STFT sqrt-Hann 75 %, *minimum statistics* (8 sub-janelas, ~1,6 s),
  ganho Wiener *decision-directed*, suavização em frequência e tempo. Sem dependências de modelo, qualquer
  taxa de amostragem, ~1 % de CPU por canal.
* **Neural**: RNNoise via `nnnoiseless` (BSD-3-Clause, modelo embutido), 48 kHz, latência 20 ms. Feature
  `rnnoise` (padrão) — pode ser desligada na compilação.
**Revisar:** modelos maiores (DeepFilterNet etc.) só com avaliação de licença, CPU e latência.

## D6 — Assinatura, distribuição, atualização e remoção do driver

**Decisão:** pacote do driver assinado via **Microsoft Hardware Dev Center** (assinatura de atestação para
Windows 10/11); app e instalador assinados com certificado EV/OV (`signtool`, ver
`packaging/windows/build-release.ps1`). Instalação por `pnputil` dentro do instalador, com backup do
dispositivo padrão e opção de reversão; remoção pelo desinstalador. Atualizações do app não reinstalam o
driver se a versão do driver não mudou.
**Estado (2026-10-14):** primeiro build feito. `platform/windows-driver/scripts/build-driver.ps1` assina
`.sys` e `.cat` com o certificado de `NOVAMIX_DRIVER_SIGN_CERT` ou, sem ele, com um certificado de teste
autoassinado, e marca o pacote com `TEST-SIGNED.txt`. `build-release.ps1` não inclui pacotes de teste no
instalador. Instalação e remoção (instalador Inno e VMs de teste) passam por
`platform/windows-driver/scripts/novamix-driver.ps1`, executado via UAC. Ele cria o devnode
`ROOT\NovaMixVirtualAudio` via SetupAPI (só o `pnputil` não cria dispositivos enumerados pela raiz), roda
`pnputil /add-driver /install`, confere os endpoints e desfaz tudo em caso de falha. Também salva os dispositivos
padrão em `%ProgramData%\NovaMix` e os restaura só nos papéis que ficaram apontando para endpoints NovaMix. A
remoção do driver é opcional na desinstalação (padrão: sim).
**Revisar:** ao submeter o pacote para atestação e depois do primeiro teste do instalador em VM.

## D7 — Plataforma de benchmark de referência

**Decisão:** máquina de desenvolvimento atual (Windows 11 Pro build 26200, x64) com headset USB e
dispositivo HD Audio onboard, e cabos VB-Audio. O comando `novamix-cli bench` e o teste
`eight_channels_fit_the_128_frame_budget` são a referência reprodutível; resultados em
[audio-performance.md](audio-performance.md).
**Revisar:** adicionar uma máquina de entrada (4 núcleos, ~2018) antes do beta.

## Decisões de engenharia

### E1 — Motor dentro do callback do dispositivo mestre
O motor roda no callback da saída mestre (como o "A1" do Voicemeeter); demais dispositivos usam pontes
com resampler e controle de deriva. Sem saída, um relógio interno de alta resolução assume.

### E2 — Plano de controle declarativo
A UI edita um `Profile`; o controlador calcula o diff e envia apenas comandos alterados. Uma única via
de código cobre edição ao vivo, troca de perfil, desfazer/refazer, importação e modo seguro.

### E3 — Todos os estágios pré-alocados
Cada canal tem todos os estágios de DSP sempre alocados; ativar, desativar ou reordenar nunca aloca.
Mudanças que alteram latência usam *duck* → troca → espera da latência → *fade-in*.

### E4 — Solo seguro por padrão
`SoloMode::MonitorOnly`: solar um canal só afeta o que você ouve; o microfone virtual e o stream nunca
são interrompidos por um solo. O modo clássico (todos os buses) é opcional.

### E5 — Proteção de rota ativa
Ciclos detectados (loopback de uma saída usada, pares de cabos virtuais) desligam automaticamente o
envio que fecha o ciclo no perfil efetivo, até o usuário corrigir. Ao tentar ativar um envio, a UI
pré-visualiza o caminho e recusa ciclos.

### E6 — Falhas no callback
Pânicos no callback são capturados (`catch_unwind`); o motor é marcado como falho, silenciado e
reconstruído pela thread controladora a partir do perfil atual.
