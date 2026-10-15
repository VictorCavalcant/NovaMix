# NovaMix

Mixer de áudio virtual para desktop, com foco em desempenho e controle — escrito em Rust.
Inspirado na categoria de mixers virtuais como o Voicemeeter: reúne roteamento, mixagem,
monitoração e processamento de voz numa interface clara, com baixa latência e uso de CPU previsível.

![status](https://img.shields.io/badge/status-MVP%20em%20desenvolvimento-orange) ![plataforma](https://img.shields.io/badge/plataforma-Windows-blue)

## O que já funciona

| Área | Situação |
|---|---|
| Motor de áudio em tempo real (sem alocação, locks ou I/O no callback) | ✅ |
| Canais (até 8) e buses (até 8), envios pré/pós-fader, mute/solo/pan/mono | ✅ |
| Pontes entre dispositivos com compensação de deriva de clock e conversão de taxa | ✅ |
| DSP de voz: HPF, redução de ruído (espectral e neural RNNoise local), gate, expansor, AGC, EQ paramétrico 5/10 bandas, de-esser, compressor, ganho, limiter com antecipação | ✅ |
| Presets: Natural, Sala ruidosa, Voz para chamada, Voz para stream, Sem processamento | ✅ |
| Bypass sem cliques com latência compensada, comparação A/B com volume compensado | ✅ |
| Detecção de ciclos de realimentação, bloqueio automático e "Corrigir rota" | ✅ |
| Perfis versionados (TOML/JSON), migração de schema, validação, backups, diff auditável | ✅ |
| Reconexão automática, notificação de dispositivos (IMMNotificationClient), dispositivo de recuperação | ✅ |
| Diagnóstico: contadores de underrun/overrun, carga do callback (p50/p95/p99), log, relatório anonimizável | ✅ |
| Interface egui: mixer, efeitos, roteamento, perfis, configurações, diagnóstico, modo compacto | ✅ |
| Assistente de configuração, ajuste assistido de voz, modo seguro, desfazer/refazer | ✅ |
| Bandeja do sistema, atalhos globais (mute, push-to-talk, perfis…), iniciar com o Windows | ✅ |
| PT-BR e inglês, tema claro/escuro, escala de interface, navegação por teclado | ✅ |
| CLI headless: dispositivos, benchmark, tom de teste, soak test, validação, diagnóstico | ✅ |
| **Driver de endpoints virtuais próprio (Windows)** | 🟡 projetado — ver [`platform/windows-driver`](platform/windows-driver/README.md). Em desenvolvimento usa cabos virtuais já instalados (VB-CABLE etc.) |
| Linux (PipeWire) / macOS (Core Audio) | 🔜 fase 6 — ver [`platform/`](platform/) |

## Começando

Pré-requisitos: Rust estável ≥ 1.95 (o `rust-toolchain.toml` fixa a versão usada), Windows 10/11 com o
toolchain MSVC.

```bash
cargo run --release -p novamix
```

Opções: `--minimized` (inicia na bandeja), `--safe-mode` (sem efeitos, rota mínima), `--no-audio` (só a interface).

Ferramentas de linha de comando:

```bash
cargo run --release -p novamix-cli -- devices
```

```bash
cargo run --release -p novamix-cli -- bench --channels 8 --block 128
```

```bash
cargo run --release -p novamix-cli -- run --seconds 1800
```

Outros comandos: `tone` (sinal de teste numa saída, sem captura), `validate <perfil>`, `diag`, `paths`.

### Endpoints virtuais

O NovaMix detecta automaticamente:

1. o driver **NovaMix** (`NovaMix Virtual Playback`, `NovaMix Mic Feed` → `NovaMix Virtual Mic`), ou
2. em desenvolvimento, cabos virtuais de terceiros já instalados (VB-Audio Virtual Cable, Hi-Fi Cable,
   VoiceMeeter, Virtual Audio Cable). O primeiro cabo vira o **microfone virtual** e o segundo, a
   **reprodução virtual**.

Detalhes em [`docs/device-installation.md`](docs/device-installation.md).

## Testes

```bash
cargo test --workspace
```

Inclui testes de DSP (resposta em frequência, tempos de ataque/liberação, latência, estabilidade),
do motor (offline, sem dispositivos), da ponte com deriva de clock simulada, do roteamento ponta a ponta
e de orçamento de processamento. Testes longos ficam marcados com `#[ignore]`:

```bash
cargo test --release -p novamix-tests -- --ignored
```

## Estrutura

```text
crates/
  dsp/             filtros, dinâmica, redução de ruído, resampler, medidores, cadeia de voz
  mixer-graph/     ids, solo, grafo de rotas, detecção de ciclos e correções
  config-model/    schema versionado, validação, migração, armazenamento, diff de perfis
  diagnostics/     métricas lock-free, fila de eventos RT, log, relatório
  audio-core/      motor em tempo real, strips, buses, pontes, relógio interno
  device-api/      abstração de dispositivos + backend CPAL (WASAPI/ASIO)
  device-windows/  endpoints virtuais, notificações, MMCSS, autostart
  control/         thread controladora, reconciliador, supervisor de streams
app/
  desktop-ui/      aplicação egui/eframe (binário `novamix`)
  cli/             ferramentas headless (binário `novamix-cli`)
platform/          driver Windows (projeto), adaptadores Linux/macOS (planejados)
packaging/windows/ instalador (Inno Setup) e script de release
docs/              arquitetura, desempenho, instalação, solução de problemas, decisões
tests/             integração, fixtures de DSP e desempenho
```

Leia [`docs/architecture.md`](docs/architecture.md) para o desenho completo e
[`docs/decisions.md`](docs/decisions.md) para as decisões registradas.

## Privacidade

O NovaMix não grava, armazena nem transmite áudio. A redução de ruído neural roda localmente.
Relatórios de diagnóstico contêm apenas dados técnicos e podem ocultar nomes de dispositivos e dados do sistema.
Relatórios de falha são gravados apenas localmente.

## Licença

MIT OR Apache-2.0. Dependências relevantes: CPAL (Apache-2.0), egui/eframe (MIT/Apache-2.0),
nnnoiseless/RNNoise (BSD-3-Clause), realfft/rustfft (MIT/Apache-2.0), rtrb (MIT/Apache-2.0).
