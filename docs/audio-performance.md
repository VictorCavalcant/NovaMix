# Desempenho de áudio

Metas do plano: processar 48 kHz com buffers de 128 frames sem alocação, bloqueios ou I/O no callback,
com percentis de tempo de processamento dentro do orçamento do bloco, e ensaios contínuos de 30 min.

## Como medir

```bash
cargo run --release -p novamix-cli -- bench --channels 8 --block 128 --preset noisy-room
```

```bash
cargo test --release -p novamix-tests --test performance
```

```bash
cargo test --release -p novamix-tests -- --ignored
```

O primeiro mede o motor offline (sem dispositivos) com tons de ruído rosa em todos os canais e três buses
com limiter. O segundo verifica que 8 canais com presets completos cabem no orçamento de 128 frames no
p99. O terceiro processa 30 minutos de áudio e verifica estabilidade numérica e ausência de vazamentos.

Em execução real, `novamix-cli run --seconds 1800` imprime a cada segundo a carga do callback
(média/p99/máx), underruns/overruns por fluxo e a latência estimada da voz.

## Resultados de referência

Máquina de referência (D7): Windows 11 Pro 26200, x64. `cargo build --release`.

| Cenário (48 kHz) | Orçamento | p50 | p99 | Máx |
|---|---|---|---|---|
| 4 canais, preset Stream, bloco 128 | 2667 µs | 84 µs (3,2 %) | 98 µs (3,7 %) | 141 µs (5,3 %) |
| 8 canais, preset Natural, bloco 64 | 1333 µs | 87 µs (6,5 %) | 112 µs (8,4 %) | 165 µs (12,4 %) |
| 8 canais, preset Sala ruidosa (neural), bloco 128 | 2667 µs | 91 µs (3,4 %) | 821 µs (30,8 %) | 1121 µs (42 %) |
| 8 canais, preset Sala ruidosa (neural), bloco 480 | 10000 µs | 1041 µs (10,4 %) | 1323 µs (13,2 %) | 1555 µs (15,6 %) |

Observações:

* O modo **neural** processa quadros de 480 amostras: com blocos pequenos a carga fica concentrada em
  um a cada ~4 callbacks (p99 alto, média baixa). Use-o nos canais de microfone, não em todos.
* O modo espectral custa ~1 % de CPU por canal em qualquer bloco.
* Em uso real com WASAPI (período de 10 ms), a carga média medida foi ~2,5 % com o perfil padrão.

## Orçamento x latência

O orçamento por bloco é `frames / taxa` (128 a 48 kHz = 2,67 ms). Isso **não** é a latência de ponta a
ponta, que soma:

| Parcela | WASAPI compartilhado via CPAL (48 kHz) | `wasapi-lowlatency`, período 128 |
|---|---|---|
| Buffer do dispositivo de entrada | 10 ms (período do Windows) | 2,7 ms |
| Ponte entrada → motor (alvo) | bloco + ½ bloco + margem (Equilibrado: ~17 ms) | ~6 ms (Baixa latência) |
| Cadeia de efeitos | NR espectral 5/11/21 ms, neural 20 ms; limiter = antecipação (2 ms) | igual |
| Bloco do motor | 10 ms | 2,7 ms |
| Ponte motor → saída não mestre | ~17 ms (zero se a saída for o relógio mestre) | ~6 ms |
| Buffer do dispositivo de saída | 10 ms | 2,7 ms |

A barra inferior mostra a estimativa "Latência da voz" (microfone → microfone virtual). Para reduzi-la:

1. **Relógio mestre = bus de voz** (Configurações → Dispositivos): remove a ponte de saída do caminho do
   microfone virtual (−15 a −20 ms).
2. Modo **Baixa latência** (margem de 2 ms nas pontes).
3. Redução de ruído espectral em "Baixa latência" (5 ms) ou desligada.
4. **Sistema de áudio `wasapi-lowlatency`** (Configurações → Dispositivos), descrito abaixo.
5. **ASIO** para interfaces profissionais: `cargo build --release -p novamix --features novamix-device-api/asio`
   (requer o ASIO SDK da Steinberg; ver documentação do CPAL). Com ASIO o período pode cair para 1–3 ms.

## Backend WASAPI próprio (`wasapi-lowlatency` / `wasapi-exclusive`)

O CPAL (sistema padrão, `wasapi`) usa WASAPI compartilhado com o período padrão do dispositivo
(normalmente 10 ms). O NovaMix tem um backend WASAPI próprio
(`crates/device-api/src/wasapi_lowlat.rs`, feature `wasapi-lowlatency`, ligada por padrão no Windows),
selecionável em **Configurações → Dispositivos → Sistema de áudio**. O CPAL continua sendo o padrão.

* **`wasapi-lowlatency`** — modo compartilhado com `IAudioClient3::InitializeSharedAudioStream`. O período
  é o tamanho de buffer do modo de latência (Baixa latência 128, Equilibrado 256, Estabilidade 512, ou o
  valor manual) arredondado para o período fundamental do driver e limitado à faixa de
  `GetSharedModeEnginePeriod`; sem pedido usa o mínimo. Drivers sem suporte a períodos pequenos ficam no
  período padrão (10 ms), como no CPAL. Se outro programa já fixou o período do endpoint, o NovaMix usa
  o período corrente. A faixa aparece em `novamix-cli devices --host wasapi-lowlatency --json`
  (`buffer_range`) e filtra a lista de tamanhos de buffer na interface.
* **`wasapi-exclusive`** — modo exclusivo (o dispositivo fica reservado ao NovaMix; outros programas não
  tocam nele) com formato nativo (f32, i32, i24 ou i16) e o período mínimo do dispositivo ou o pedido.
  Loopback é sempre compartilhado. Dispositivos que recusam o modo exclusivo (ocupados, política, sem
  formato nativo) abrem em compartilhado de baixa latência. Cada stream exclusivo é verificado durante
  ~1 s: se não entrega um período por evento (taxa fora de ±10 % ou mais de 5 % de eventos atrasados mais
  de 2 períodos — visto com o VB-Audio Hi-Fi Cable, cujos eventos seguem o timer de 15,6 ms), ele é
  reaberto em compartilhado e o dispositivo não é mais tentado em exclusivo nesta sessão.

Comum aos dois: uma thread por stream, orientada a eventos, registrada no MMCSS "Pro Audio"; conversão
para `f32` intercalado com buffers alocados antes do início; o render mantém no máximo dois períodos na
fila do dispositivo; dispositivo removido ou serviço de áudio reiniciado (`AUDCLNT_E_DEVICE_INVALIDATED`,
`AUDCLNT_E_SERVICE_NOT_RUNNING`) vira `DeviceErrorKind::Disconnected` (também detectado sem eventos, por
sondagem a cada 500 ms) e descontinuidades de captura viram `Xrun`. Os ids de dispositivo são os mesmos do
CPAL (`wasapi:{id do endpoint}`), então os perfis continuam válidos ao trocar de sistema.

O controlador pergunta ao backend o período antes de abrir cada stream (`AudioBackend::period_hint`) e
dimensiona as pontes para ele; se o período concedido for outro, a ponte é refeita. Para o CPAL nada muda
(sem dica: continua assumindo ≥ 10 ms).

Para medir sem alterar a configuração: `novamix-cli run --host wasapi-lowlatency --latency low`
(`--host`/`--latency` valem só para aquela execução).

### Antes/depois (máquina de referência D7, 2026-10-14)

Perfil "Padrão": microfone HDA → NR neural + gate + EQ → bus de voz no VB-CABLE (microfone virtual de
desenvolvimento), relógio mestre = alto-falantes HDA, Desktop pelo VB-Audio Hi-Fi Cable. Períodos
suportados nesta máquina (`GetSharedModeEnginePeriod`): alto-falantes, microfone HDA e CABLE Input
128–480 frames; Hi-Fi Cable, CABLE Output e webcams só 480.

| Sistema | Modo de latência | Bloco do motor | "voice" estimada (`novamix-cli run`) |
|---|---|---|---|
| `wasapi` (CPAL) | Equilibrado | 480 | **93,0 ms** |
| `wasapi` (CPAL) | Baixa latência | 480 | **87,0 ms** |
| `wasapi-lowlatency` | Equilibrado (período 256) | 256 | **65,0 ms** |
| `wasapi-lowlatency` | Baixa latência (período 128) | 128 | **43,0 ms** |
| `wasapi-exclusive` | Baixa latência (128; Hi-Fi Cable volta a compartilhado) | 128 | **43,0 ms** |

Carga do callback em todos os casos: média 2–3 %, p99 ≤ 10 %. A estimativa não inclui a fila interna do
dispositivo de saída (o CPAL enche o buffer inteiro; o backend próprio mantém ≤ 2 períodos), então a
diferença real é um pouco maior que a da tabela. Com períodos iguais o modo exclusivo não muda a
estimativa; ele só tira o mixer do Windows do caminho.

Ensaio contínuo (`novamix-cli run --seconds 1800 --host wasapi-lowlatency`, Equilibrado, 30 min):
microfone → voz com **0 underruns e 0 overruns**, carga média 1,9 %, p99 10 %, máx. 21 %, "voice" estável em
65,0 ms. O canal Desktop (captura do VB-Audio Hi-Fi Cable, preso em 480 frames) teve 293 underruns
(~0,16/s), concentrados quando algo tocava no cabo — problema que já existia com o CPAL (11 em 20 s,
~0,55/s, na mesma máquina) e vem do ritmo irregular desse cabo, não do backend. Um ensaio anterior em
Baixa latência rodou 461 s com o mesmo resultado no microfone (0/0) antes de ser interrompido por fora.

## Regras verificadas por testes

* Sem alocação no callback: estruturas pré-alocadas; objetos substituídos voltam pela fila de lixo
  (`invalid_indexes_are_ignored_and_boxes_returned`, `input_bridge_feeds_channel_and_replaced_reader_is_returned`).
* Saída independente do tamanho do bloco (`output_is_independent_of_block_size`).
* Mudanças sem cliques: fader/mute/pan (`fader_mute_and_pan_are_smooth`), ligar redução de ruído
  (`toggling_noise_reduction_ducks_instead_of_clicking`), bypass (`bypass_is_latency_compensated_and_click_free`),
  coeficientes de EQ (`coefficient_ramp_has_no_discontinuity`).
* Deriva de relógio de 300 ppm por 120 s simulados sem underruns (`drift_controller_converges_without_glitches`).
* Silêncio, DC, clipping e denormais não geram NaN nem picos acima do teto
  (`silence_dc_and_clipped_input_stay_finite_and_bounded`).

## Ajustes de economia

O **modo economia** reduz os medidores para 15 fps e desliga o analisador de espectro; com a janela oculta
a UI atualiza a cada 250 ms. Nada disso afeta o caminho de áudio.
