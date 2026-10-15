# Empacotamento Windows

```bash
powershell -ExecutionPolicy Bypass -File packaging/windows/build-release.ps1
```

Gera em `dist/`:

| Artefato | Descrição |
|---|---|
| `NovaMix/` | binários `novamix.exe` e `novamix-cli.exe`, README e docs |
| `NovaMix-<versão>-portable.zip` | versão portátil (contém `portable.txt`: dados ficam ao lado do executável) |
| `NovaMix-<versão>-setup.exe` | instalador Inno Setup 6 (se instalado) |
| `SHA256SUMS.txt` | somas para verificação de integridade |

## Assinatura

Defina `NOVAMIX_SIGN_CERT` com a impressão digital (SHA-1) do certificado de assinatura de código no
repositório de certificados do usuário. O script usa `signtool` do Windows SDK com carimbo de tempo
RFC 3161. Sem a variável, o build é marcado como **não assinado**.

## Instalador

* Instalação por usuário (sem administrador) por padrão; o usuário pode escolher instalar para todos.
* Tarefas opcionais: atalho na área de trabalho, iniciar com o Windows (`--minimized`, na bandeja).
* O componente **driver** só aparece quando `platform/windows-driver/package/NovaMixVirtualAudio.inf`
  (pacote assinado) existe. Ao instalar, o instalador explica por que pede administrador e roda
  `driver\novamix-driver.ps1 -Action Install` via UAC. O script salva os dispositivos padrão, cria o dispositivo
  `ROOT\NovaMixVirtualAudio`, roda `pnputil`, confere os três endpoints, desfaz tudo em caso de falha e
  restaura os padrões. Na desinstalação, pergunta se o driver deve ser removido (`-Action Uninstall`). Logs em
  `%ProgramData%\NovaMix\logs`. Detalhes: `platform/windows-driver/README.md`, seção "Instalação no produto".
  Pacotes assinados com certificado de teste (`TEST-SIGNED.txt`) só entram com
  `build-release.ps1 -IncludeTestSignedDriver`.
* O `novamix.iss` compila (Inno Setup 6.7.3, com e sem o pacote do driver, sem avisos). O
  `build-release.ps1` também encontra o Inno Setup instalado por usuário (`%LOCALAPPDATA%\Programs`). A
  instalação real do driver pelo instalador ainda precisa ser testada numa VM.
* A desinstalação mantém `%APPDATA%\NovaMix` a menos que o usuário confirme a remoção.
* Atalho extra "NovaMix (modo seguro)" para recuperação.

## Reverter uma atualização

Instale a versão anterior por cima (as configurações são compatíveis dentro do mesmo `schema_version`;
versões mais novas recusam arquivos de schema futuro com mensagem clara). Backups automáticos ficam em
`%APPDATA%\NovaMix\backups`.
