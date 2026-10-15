# Solução de problemas

Comece por **Diagnóstico** (botão 🔍 na barra superior): estado de cada fluxo, formato negociado,
underruns/overruns, deriva de relógio, carga do callback e eventos recentes. O botão
*Exportar relatório…* gera um JSON sem áudio (com opção de ocultar nomes de dispositivos) para
compartilhar.

## Não ouço nada

* Verifique o estado do bus *Monitor*: deve estar "conectado". Se estiver "indisponível", escolha outra
  saída ou defina uma **saída de recuperação** em *Configurações → Dispositivos*.
* Confirme que o canal tem envio para *Monitor* (botão `MONI` aceso) e não está em mudo/solo de outro canal.
* Um *cue* ativo em outro bus substitui a mixagem do monitor.
* O modo seguro mantém a rota, mas o modo de teste (Diagnóstico) troca o último canal por um bipe.

## Os apps não recebem meu microfone

* Nos apps, selecione o endpoint indicado no assistente (driver NovaMix: `NovaMix Virtual Mic`; fallback:
  o lado *Output* do cabo, ex. `CABLE Output`).
* O canal do microfone precisa enviar para o bus *Voz* (`VOZ` aceso) e o bus *Voz* deve ter saída
  "Microfone virtual NovaMix".
* Se aparecer "Sem endpoints virtuais", instale o driver ou um cabo virtual (ver
  [device-installation.md](device-installation.md)).

## Eco ou microfonia

* O retorno do microfone para o monitor vem **desligado** por padrão; ao ligá-lo o NovaMix pede
  confirmação. Use fones de ouvido.
* "Ciclo de áudio bloqueado" na barra inferior: alguma saída está sendo capturada de novo (loopback ou
  cabo). Abra *Roteamento* e use **Corrigir rota**.

## Estalos, cortes ou underruns

* Mude para o modo **Estabilidade** (margem maior nas pontes).
* Veja a carga em *Diagnóstico*: p99 acima de ~70 % indica CPU no limite — reduza a qualidade da redução de
  ruído (espectral "Baixa latência"), use o modo neural apenas no microfone ou desative canais sem uso.
* Dispositivos Bluetooth costumam ter relógios instáveis; a ponte compensa deriva, mas USB com fio é mais
  previsível.
* A deriva mostrada em *Diagnóstico* acima de ±2000 ppm indica relógio do dispositivo muito instável.

## Latência alta

* No WASAPI compartilhado o Windows define o período (normalmente 10 ms) por dispositivo; cada ponte entre
  relógios diferentes adiciona ~1–2 períodos.
* Escolha o bus de voz como **Relógio mestre** para remover uma ponte do caminho do microfone virtual.
* Redução de ruído adiciona latência: espectral 5/11/21 ms (baixa latência/equilibrada/alta qualidade),
  neural 20 ms; o limiter adiciona a antecipação configurada (padrão 2 ms).
* Em Configurações → Dispositivos, troque o **Sistema de áudio** para `wasapi-lowlatency`: em dispositivos
  com driver compatível o período cai de 10 ms para o tamanho de buffer do modo de latência (até 128
  frames / 2,7 ms). `wasapi-exclusive` reserva os dispositivos para o NovaMix (outros programas ficam sem
  som neles).
* Para latências muito baixas em interfaces profissionais, compile com ASIO (ver
  [audio-performance.md](audio-performance.md)).

## Dispositivo desconectado

A reconexão é automática (intervalo configurável, com back-off até 30 s). Quando o id do dispositivo
muda (outra porta USB), o NovaMix encontra o dispositivo pelo nome. O log registra "device connected" e
"input ... opened".

## O aplicativo não abre / fecha sozinho

* Apenas uma instância roda por vez (procure o ícone na bandeja).
* Inicie com `--safe-mode` para desligar efeitos ou `--no-audio` para abrir só a interface.
* Relatórios de falha locais ficam em `%APPDATA%\NovaMix\diagnostics\crash-*.txt`.
* Configuração corrompida é renomeada para `config.corrupt-<data>.toml` e os padrões são carregados.
* Backups: *Configurações → Geral → Backups*.

## Logs

`%APPDATA%\NovaMix\logs\novamix.log.<data>` — o nível pode ser alterado em
*Configurações → Diagnóstico* ou pela variável `NOVAMIX_LOG` (ex.: `NOVAMIX_LOG=debug`).
