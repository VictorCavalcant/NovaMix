# Guia rápido

## Primeira execução

O **assistente** abre automaticamente:

1. Idioma.
2. **Microfone**: escolha o dispositivo e fale — o medidor deve se mover e aparecer "Nível bom".
3. **Saída**: escolha onde ouvir (de preferência fones) e toque o bipe de teste.
4. **Endpoints virtuais**: o assistente diz qual dispositivo selecionar como microfone nos apps
   (Discord, Zoom, OBS…) e para onde enviar o som de apps que você queira mixar.
5. **Preset de voz**: Natural, Reunião (Voz para chamada), Jogo (Sala ruidosa) ou Stream.
6. Resumo da rota. Você pode reabrir o assistente pelo botão ✨ a qualquer momento.

## A janela principal

* **Canais** (esquerda): nome (duplo clique renomeia), fonte, estado, ganho de entrada, medidores (entrada e
  saída), fader (arraste, roda do mouse, setas do teclado; Shift = ajuste fino; duplo clique = 0 dB), pan,
  **M** (mudo), **S** (solo), **Mono**, **FX** (liga/desliga todos os efeitos), envios para os buses.
  Clique com o botão direito para copiar efeitos ou remover o canal.
* **Buses** (direita dos canais): Monitor, Voz e Stream por padrão — saída, fader, mudo, mono, **Cue**
  (pré-escuta no monitor), atraso e limiter de segurança.
* **Efeitos** (painel lateral): cadeia do canal selecionado. Modo simples: um controle por efeito; modo
  avançado (Configurações → Voz e efeitos): todos os parâmetros, reordenação, EQ de 10 bandas.
  **Ouvir original (A/B)** compara com o sinal sem efeitos, com volume compensado.
* **Barra superior**: estado do motor, perfil ativo, desfazer/refazer, **Modo seguro** e indicador de CLIP.
* **Barra inferior**: latência estimada da voz, CPU de áudio, underruns, estado dos endpoints virtuais e
  avisos de rota.

## Configurar uma chamada típica

1. Perfil **Reunião** (Perfis → Novo a partir de modelo → Reunião, ou carregue-o): mostra antes o que vai
   mudar.
2. Microfone → bus **Voz** (já ligado). No app de chamada, selecione o microfone virtual.
3. Áudio da chamada: deixe o app tocar nos seus fones normalmente, ou envie-o para a reprodução virtual se
   quiser controlá-lo no NovaMix.
4. Use o **Ajuste de voz** (🎤): 5 s de silêncio + 8 s falando → sugestões de ganho, gate, expansor e
   compressor, aplicadas só se você confirmar.

## Atalhos

* Na janela: **Ctrl+Z** desfazer, **Ctrl+Y** refazer, **Ctrl+M** silenciar microfone, **Tab** navega entre
  controles.
* Globais (opcionais, Configurações → Atalhos): silenciar microfone, pressionar para falar/silenciar,
  próximo/anterior perfil, ativar perfil, silenciar bus, modo seguro, mostrar/ocultar janela.

## Bandeja do sistema

Clique no ícone para abrir a janela; menu com silenciar microfone, modo seguro, troca de perfil e sair.
"Minimizar para a bandeja" e "Fechar para a bandeja" ficam em Configurações → Geral.

## Recuperação

* **Modo seguro**: desliga todos os efeitos e o retorno do microfone, mantendo a rota. Clique de novo
  para voltar — o perfil não é alterado.
* **« Perfil anterior**: volta à configuração anterior à última troca de perfil.
* **Backups**: Configurações → Geral → Criar/Restaurar backup.
* Iniciar com `--safe-mode`: atalho "NovaMix (modo seguro)" no menu Iniciar.
