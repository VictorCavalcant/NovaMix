# Changelog

## 0.1.0 — MVP (em desenvolvimento)

### Adicionado
- Motor de áudio em tempo real (`novamix-audio-core`): até 8 canais e 8 buses, envios pré/pós-fader,
  mute/solo/pan/mono, cue de bus, atraso por bus, limiter de segurança, saída mestre e pontes com
  compensação de deriva de relógio e conversão de taxa.
- DSP (`novamix-dsp`): HPF 6–24 dB/oct, redução de ruído espectral e neural (RNNoise local), gate com
  histerese e hold, expansor, AGC, EQ paramétrico 5/10 bandas, de-esser, compressor com joelho suave,
  ganho de compensação, limiter com antecipação, resampler sinc de razão variável, medidores e espectro.
- Presets de voz: Natural, Sala ruidosa, Voz para chamada, Voz para stream, Sem processamento.
- Perfis versionados (TOML/JSON) com migração, validação, backups automáticos e diff auditável.
- Detecção de ciclos de realimentação com bloqueio automático e "Corrigir rota".
- Controlador com reconexão automática, notificações de dispositivos, dispositivo de recuperação e
  relógio interno.
- Interface egui (PT-BR/EN, temas, escala): mixer, efeitos, roteamento, perfis, configurações,
  diagnóstico, assistente, ajuste assistido de voz, modo compacto, modo seguro, desfazer/refazer,
  bandeja do sistema e atalhos globais.
- CLI headless: devices, bench, tone, run, validate, diag, paths (`run --host/--latency` para medir sem
  alterar a configuração).
- Backend WASAPI próprio de baixa latência: sistemas `wasapi-lowlatency` (IAudioClient3, períodos abaixo
  de 10 ms) e `wasapi-exclusive` (modo exclusivo com volta automática para compartilhado); pontes
  dimensionadas pelo período real (`AudioBackend::period_hint`). CPAL segue como padrão.
- Endpoints virtuais: contrato do driver NovaMix e fallback para cabos virtuais de terceiros.
- Documentação, plano do driver Windows, adaptadores Linux/macOS planejados, instalador Inno Setup.
