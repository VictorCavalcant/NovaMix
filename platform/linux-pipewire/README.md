# Adaptador Linux (PipeWire) — fase 6

> **Status:** planejado. O motor, o DSP, a configuração e a UI já são portáveis; falta o adaptador de
> plataforma e a validação.

## Estratégia

O PipeWire já cria *sinks* e *sources* virtuais sem driver de kernel, com os módulos
[loopback](https://docs.pipewire.org/page_module_loopback.html) e
[filter-chain](https://docs.pipewire.org/page_module_filter_chain.html). O adaptador Linux deve:

1. Criar (ou instalar como drop-in de configuração) os mesmos três nós do contrato Windows, para que a
   classificação de endpoints (`virtual_endpoints.rs`) funcione sem mudanças:
   * `NovaMix Virtual Playback` — *null sink* (apps tocam aqui; o NovaMix lê o monitor do sink);
   * `NovaMix Mic Feed` → `NovaMix Virtual Mic` — loopback sink → source.
2. Usar o backend PipeWire do CPAL (`cpal` com a feature `pipewire`) para enxergar os nós individualmente.
3. Respeitar o gerenciador de sessão (WirePlumber): não forçar links; deixar o usuário escolher nos apps.
4. Implementar `DeviceWatcher` com eventos do registry do PipeWire e `boost_current_thread` com
   rtkit/`audio_thread_priority`.

## Configuração de exemplo

O arquivo [`novamix-virtual.conf`](novamix-virtual.conf) cria os nós manualmente para testes:

```bash
mkdir -p ~/.config/pipewire/pipewire.conf.d && cp novamix-virtual.conf ~/.config/pipewire/pipewire.conf.d/
```

```bash
systemctl --user restart pipewire
```

## Critérios para suporte

* Teste em Fedora (PipeWire + WirePlumber) e Ubuntu LTS.
* `novamix-cli devices` reconhece os três nós; `novamix-cli run` por 30 min sem underruns.
* Instalação por pacote (Flatpak ou .deb/.rpm) que adicione/remova o drop-in.
