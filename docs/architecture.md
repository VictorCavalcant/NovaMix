# Arquitetura

O NovaMix é **um motor de áudio em tempo real + uma camada de dispositivos por plataforma + uma interface
configurável**. Este documento descreve como as peças se encaixam e quais regras garantem áudio sem
estalos.

## Visão geral

```text
┌──────────────────────────── processo NovaMix ─────────────────────────────┐
│                                                                           │
│  UI (egui, thread principal)                                              │
│    edita um Profile ──► ControllerHandle ──(canal)──► thread controladora │
│    lê EngineStatus / MeterBank / EventLog ◄─────────────── publica        │
│                                                                           │
│  thread controladora (novamix-control)                                    │
│    • resolve fontes/saídas → dispositivos concretos                       │
│    • analisa o grafo de rotas (ciclos) e bloqueia envios perigosos        │
│    • abre/fecha streams, reconecta, escolhe o relógio mestre              │
│    • reconcilia Profile antigo × novo → comandos mínimos                  │
│         │ fila SPSC lock-free (rtrb)            ▲ fila de "lixo" (Box)    │
│         ▼                                       │                         │
│  thread de áudio mestre (callback do dispositivo de saída mestre          │
│  ou relógio interno)                                                      │
│    MixerCore: strips → envios → buses → saídas                           │
│         ▲ pontes SPSC com resampler + controle PI de deriva ▼             │
│  callbacks de entrada (CaptureSink)        callbacks de outras saídas     │
│                                            (PlaybackSource)               │
└───────────────────────────────────────────────────────────────────────────┘
```

## Componentes (crates)

| Componente do plano | Crate | Responsabilidade |
|---|---|---|
| DSP Graph | `novamix-dsp` | Processadores in-place (mono/estéreo), `VoiceChain`, resampler, medidores, analisador de espectro |
| Mixer e roteamento | `novamix-mixer-graph` | Limites, solo, grafo fonte→canal→bus→saída com ligações de sistema (loopback, cabos virtuais), Tarjan SCC, sugestões de correção |
| Configuração | `novamix-config` | `AppConfig`/`Profile` versionados, validação/saneamento, migração v0→v1, gravação atômica, backups, diff auditável |
| Diagnostics | `novamix-diagnostics` | Contadores atômicos, histograma de carga do callback, fila de eventos RT sem bloqueio, log limitado, relatório anonimizável |
| Audio Engine | `novamix-audio-core` | `MixerCore`, `ChannelStrip`, `BusStrip`, comandos, pontes, relógio interno, `MeterBank` |
| Device Layer | `novamix-device-api` | `AudioBackend` (enumeração, streams f32 intercalados), backend CPAL (WASAPI, ASIO opcional), backend WASAPI próprio de baixa latência (`wasapi-lowlatency`/`wasapi-exclusive`), `NullBackend` |
| Virtual Device Adapter | `novamix-device-windows` | Detecção de endpoints NovaMix ou cabos de terceiros, `IMMNotificationClient`, MMCSS, autostart, versão do SO |
| Control Plane | `novamix-control` | Thread controladora, reconciliador, resolução de dispositivos, supervisão/reconexão, status, relatório |
| Desktop UI | `novamix` (app/desktop-ui) | Mixer, efeitos, roteamento, perfis, configurações, diagnóstico, assistente, bandeja, atalhos |
| Ferramentas | `novamix-cli` | Benchmark, soak test, tom de teste, validação, diagnóstico |

## Fluxo de um bloco de áudio

1. O callback do dispositivo mestre chama `MasterDriver::on_output`, que faz `try_lock` (nunca bloqueia)
   no `MixerCore`. Se o núcleo estiver indisponível (troca de motor), a saída é silêncio.
2. `MixerCore::process_master` divide o buffer do dispositivo em sub-blocos de até `max_block` frames.
3. Para cada sub-bloco:
   1. aplica até 1024 comandos pendentes (parâmetros, troca de pontes);
   2. cada canal ativo lê sua fonte (ponte de entrada ou tom de teste), detecta clipping **antes** do ganho,
      aplica ganho, mono, mede, processa a `VoiceChain`, pan, fader e mute (todos com rampa);
   3. os envios (com ganho já resolvido para solo) somam os canais nos buses;
   4. cada bus aplica fader, mute, mono, atraso e limiter de segurança e mede;
   5. cada saída soma os buses atribuídos; o bus de monitor pode ser substituído pelos buses em *cue*;
   6. a saída mestre é escrita direto no buffer do dispositivo; as demais vão para pontes.
4. O tempo do bloco é registrado no histograma (`TimingStats`); acima de 100 % do orçamento gera evento.

## Pontes entre relógios independentes

Cada dispositivo tem seu próprio relógio. `bridge()` cria um anel SPSC de quadros estéreo; o lado
consumidor tem um resampler sinc polifásico de razão variável (48 taps, 256 fases, Kaiser β = 7,5) e um
controlador PI que mantém o preenchimento do anel no alvo (correção limitada a ±0,5 %). Isso absorve
44,1↔48 kHz e deriva de ±100–300 ppm sem estalos e sem latência crescente.

* **Priming**: a leitura só começa quando o anel atinge o alvo; o excesso inicial é descartado.
* **Aquecimento**: nos primeiros 500 ms, falta de dados não conta como underrun (jitter de partida).
* **Loopback ocioso**: em captura de loopback, ausência de dados (nada tocando) é "ocioso", não erro.
* **Ressincronização**: se o anel acumular mais de 3× o alvo, o excesso é descartado (latência limitada).

O alvo é derivado dos tamanhos de bloco dos dois lados mais uma margem do modo de latência
(Baixa latência 2 ms, Equilibrado 5 ms, Estabilidade 20 ms).

## Regras de tempo real (e como são cumpridas)

| Regra | Implementação |
|---|---|
| Não alocar/liberar no callback | Tudo pré-alocado em `create_engine`/`VoiceChain::new`; objetos substituídos voltam pela fila de lixo; se a fila encher, `mem::forget` (contado) |
| Sem mutex bloqueante, disco, log, rede | Comandos por `rtrb`; eventos por fila com descarte; medidores por atômicos; `try_lock` apenas |
| Pré-alocar cadeias e trocar por mensagens | Todos os estágios existem sempre; reordenar é trocar um array; os planos FFT das três qualidades de NR são criados no início |
| UI responsiva | UI só lê atômicos/snapshots; abrir dispositivos acontece na thread controladora |
| Comportamento em falhas | Underrun → silêncio + re-priming; dispositivo removido → reconexão com back-off; pânico no callback → `catch_unwind`, motor marcado como falho e reconstruído |
| Telemetria fora do caminho crítico | Espectro via fila com perdas; FFT na UI |
| Rampa em parâmetros | `LinearSmoother` em ganhos, pan, mute e envios; coeficientes de biquad interpolados em 64 amostras; estágios sem latência fazem crossfade; estágios com latência (NR, antecipação do limiter, ordem) fazem *duck* → troca em silêncio → espera a latência → *fade-in* |

Além disso, o callback ativa *flush-to-zero/denormals-are-zero* (evita lentidão com sinais decaindo) e as
threads próprias pedem MMCSS "Pro Audio".

## Plano de controle declarativo

A UI nunca envia comandos de baixo nível: ela edita um `Profile`. A cada quadro, se o perfil mudou, ele é
enviado ao controlador, que:

1. resolve `SourceSpec`/`OutputSpec` em dispositivos (id estável, com *fallback* por nome quando o id muda);
2. monta o `GraphInput` e roda a análise de ciclos; envios que fecham ciclos são desligados no perfil
   efetivo (o usuário vê o aviso e o botão **Corrigir rota**);
3. decide se o motor precisa reiniciar (mudança do dispositivo mestre/taxa) ou só ajustes incrementais;
4. abre/fecha streams de entrada/saída e atribui *slots* de saída;
5. chama `reconcile(old, new)`, que compara campo a campo e envia apenas o que mudou.

O modo seguro, o modo de teste e o A/B são sobreposições: o perfil editado não muda.

## Relógio mestre

Por padrão, o dispositivo do bus de monitor é o mestre (monitoração com menor latência). Pode-se escolher
outro bus em *Configurações → Dispositivos → Relógio mestre* — escolher o bus de voz elimina uma ponte
no caminho do microfone virtual. Sem saída disponível, o motor roda no relógio interno (timer de alta
resolução), alimentando todas as saídas por pontes.

## Persistência

`%APPDATA%\NovaMix\` (ou `data\` ao lado do executável se existir `portable.txt`):

```text
config.toml            configurações gerais (schema_version = 1)
profiles/*.toml        perfis
backups/auto/          cópias automáticas a cada gravação (10 por arquivo)
backups/<timestamp>/   backups manuais completos
logs/                  logs diários (rotacionados)
diagnostics/           relatórios exportados e relatórios de falha locais
```

Gravações são atômicas (arquivo temporário + `fsync` + `rename`). Arquivos corrompidos são preservados
com outro nome e substituídos pelos padrões, com aviso no log.
