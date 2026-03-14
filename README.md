# Assinador Livre RS (Windows)

Aplicacao desktop em Rust para assinatura digital de PDF com certificado A3 via Windows Certificate Store.

> **Migracao de nome:** o projeto foi renomeado de `assinador-livre` para `assinador-livre-rs` para evitar colisao de nome.
>
> **Atencao para GitHub Actions:** repositorios renomeados nao mantem redirecionamento para referencias `uses: owner/repo@ref`; consumidores precisam atualizar manualmente.

## O que este app faz

- Roda residente na bandeja do Windows.
- Menu com:
  - `Assinar documento`
  - `Abrir playground`
  - `Sair`
- Assina PDFs com certificado do repositorio `MY` (Minhas) do Windows.
- Expoe WebSocket local para uma aplicacao web detectar o app e solicitar assinatura.
- Cria configuracao em `%APPDATA%\\AssinadorLivre\\config.json`.
- Registra auto-start no login do Windows (HKCU Run) quando habilitado.
- Gera logs em `%APPDATA%\\AssinadorLivre\\logs\\assinador.log` com rotacao simples por tamanho.

## Requisitos

- Windows
- Rust toolchain (para desenvolvimento)
- Middleware/driver do token A3 instalado
- Certificado com chave privada disponivel no repositorio `MY`

## Instalacao para usuario final (MSI)

Baixe o instalador na pagina de Releases:

- [Releases do Assinador Livre RS](https://github.com/celsowm/assinador-livre-rs/releases)

Se a pagina estiver vazia, ainda nao existe versao publicada para download.

Arquivo esperado em cada release:

- `assinador-livre-rs-<versao>-x64.msi`

Fluxo recomendado:

1. Execute o `.msi`.
2. Escolha o escopo de instalacao:
   - por usuario (sem admin), ou
   - por maquina (pode exigir admin).
3. Finalize a instalacao e mantenha marcada a opcao para abrir o app ao concluir.
4. No primeiro inicio, o app cria/valida configuracao e auto-start.

## Build

```powershell
cargo build --release
```

Binario esperado:

```text
target\\release\\assinador-livre-rs.exe
```

## Build do instalador MSI (desenvolvimento)

```powershell
cargo install cargo-wix --locked
cargo wix --target x86_64-pc-windows-msvc
```

Saida esperada:

```text
target\\wix\\*.msi
```

## Modos de execucao (CLI)

```powershell
# Modo bandeja (default)
assinador-livre-rs.exe

# Fluxo imediato de assinatura (abre seletor de PDF e encerra)
assinador-livre-rs.exe --sign-now

# Mostra caminho do config
assinador-livre-rs.exe --print-config-path

# Logs mais verbosos
assinador-livre-rs.exe --verbose
```

## Configuracao

Na primeira execucao, o app cria:

```text
%APPDATA%\\AssinadorLivre\\config.json
```

Exemplo:

```json
{
  "ws_host": "127.0.0.1",
  "ws_port": 45890,
  "ws_path": "/ws",
  "ws_token": "troque-este-token",
  "allowed_origins": [
    "http://localhost:3000",
    "https://seu-dominio.com"
  ],
  "cert_override": {
    "mode": "auto",
    "thumbprint": null,
    "index": null
  },
  "startup_with_windows": true
}
```

### Regras de certificado (`cert_override`)

- `mode=auto`: usa ranking automatico e prioriza certificado de token/smart card quando existir.
- `mode=token_only`: exige certificado de token/smart card; falha se nao encontrar.
- `thumbprint` preenchido: tenta certificado especifico; se nao encontrar, cai para auto.
- `index` preenchido: forca indice (1-based) da lista de certificados.

## WebSocket local

Endpoint padrao:

```text
ws://127.0.0.1:45890/ws
```

Playground local (HTTP, mesma porta do WS):

```text
http://127.0.0.1:45890/playground
```

### Regras de seguranca

- Bind apenas em localhost (`ws_host`).
- `Origin` deve estar em `allowed_origins`.
- A origem local do playground (`http://<ws_host>:<ws_port>`) e aliases locais comuns (`localhost`/`127.0.0.1`) tambem sao aceitos.
- Primeira mensagem obrigatoriamente `auth` em ate 3 segundos.
- Token deve bater com `ws_token`.

### Acoes suportadas

1. `auth`
2. `ping`
3. `sign_pdf`
4. `sign_pdfs` (assinatura em lote)

### Formato de requisicao

```json
{"id":"1","action":"auth","payload":{"token":"..."}}
```

```json
{"id":"2","action":"ping","payload":{}}
```

```json
{"id":"3","action":"sign_pdf","payload":{"filename":"doc.pdf","pdf_base64":"..."}}
```

Com assinatura visivel opcional na primeira pagina:

```json
{
  "id":"3",
  "action":"sign_pdf",
  "payload":{
    "filename":"doc.pdf",
    "pdf_base64":"...",
    "visible_signature":{
      "placement":"top_left_horizontal",
      "style":"default",
      "timezone":"local"
    }
  }
}
```

Valores aceitos em `visible_signature.placement`:

- `top_left_horizontal`
- `top_left_vertical`
- `top_right_horizontal`
- `top_right_vertical`
- `bottom_left_horizontal`
- `bottom_left_vertical`
- `bottom_right_horizontal`
- `bottom_right_vertical`
- `bottom_center_horizontal`
- `bottom_center_vertical`
- `center_horizontal`
- `center_vertical`

Valores aceitos em `visible_signature.style` (opcional):

- `default`
- `compact`

Valores aceitos em `visible_signature.timezone` (opcional):

- `local` (padrao)
- `utc`

Observacoes:

- `visible_signature` e opcional.
- `style` e opcional e usa `default` quando ausente.
- `timezone` e opcional e usa `local` quando ausente.
- Quando ausente, a assinatura continua invisivel (comportamento legado).
- Quando presente, a assinatura visivel e aplicada apenas na primeira pagina.

#### Assinatura em lote (`sign_pdfs`)

```json
{
  "id":"4",
  "action":"sign_pdfs",
  "payload":{
    "files":[
      {"filename":"doc1.pdf","pdf_base64":"...","visible_signature":{"placement":"top_left_horizontal"}},
      {"filename":"doc2.pdf","pdf_base64":"..."}
    ]
  }
}
```

Resposta (sucesso):

```json
{
  "id":"4",
  "ok":true,
  "result":{
    "files":[
      {"filename":"doc1.pdf","ok":true,"signed_pdf_base64":"..."},
      {"filename":"doc2.pdf","ok":false,"error":"mensagem de erro"}
    ],
    "cert_subject":"...",
    "cert_issuer":"...",
    "cert_thumbprint":"...",
    "cert_is_hardware_token":true,
    "cert_provider_name":"..."
  }
}
```

Observacoes:

- O certificado e carregado uma unica vez para todo o lote.
- Cada arquivo reporta sucesso/falha individualmente.
- O limite de `pdf_base64` (20 MB) e aplicado por arquivo.
- O semaforo de assinatura e adquirido uma vez para o lote inteiro.

### Formato de resposta (sucesso)

```json
{"id":"3","ok":true,"result":{"signed_pdf_base64":"...","cert_subject":"...","cert_issuer":"..."}}
```

### Formato de resposta (erro)

```json
{"id":"3","ok":false,"error":{"code":"SIGNING_FAILED","message":"..."}}
```

### Codigos de erro

- `AUTH_REQUIRED`
- `AUTH_FAILED`
- `ORIGIN_NOT_ALLOWED`
- `INVALID_REQUEST`
- `SIGNING_FAILED`
- `BUSY`

### Limites operacionais

- `pdf_base64`: max 20 MB.
- Timeout de autenticacao: 3s.
- Timeout de assinatura: 120s.

## Playground WebSocket local

Use o endpoint HTTP local para testar o protocolo sem app web externa:

1. Inicie o app em modo bandeja.
2. Abra `http://127.0.0.1:45890/playground` no navegador.
3. Clique em `Conectar`.
4. Clique em `Autenticar` (token predefinido para dev: `troque-este-token`).
5. Teste `Ping` e `Assinar PDF`.

Observacao importante:

- O token predefinido do playground e apenas para desenvolvimento local.
- Em producao, altere `ws_token` no `config.json`.

## Fluxo de bandeja

1. Inicie o app sem argumentos.
2. Clique direito no icone da bandeja.
3. Clique em `Abrir playground` para abrir `http://127.0.0.1:45890/playground` no navegador; ou clique em `Assinar documento`.
4. Selecione um ou mais PDFs.
5. Arquivos assinados sao gravados como `*_assinado.pdf` no mesmo diretorio.

## Auto-start no Windows

Quando `startup_with_windows=true`, o app garante entrada em:

```text
HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run
```

Valor:

```text
AssinadorLivre = "<caminho-do-exe>"
```

No instalador MSI, a chave de auto-start tambem e criada no `HKCU Run` durante a instalacao.
Assim, o comportamento fica redundante por seguranca:

- o MSI grava a entrada para o usuario instalador;
- o proprio app revalida/atualiza a entrada ao iniciar.

## Release automatico no GitHub

O workflow `.github/workflows/release-msi.yml` publica release estavel ao receber `push` na `main`.

Contrato operacional:

- cada novo release exige bump em `version` no `Cargo.toml`;
- se a versao atual ja existir no ultimo release estavel, o workflow falha;
- o release e publicado com tag `v<versao>`;
- o asset publicado segue o padrao `assinador-livre-rs-<versao>-x64.msi`.

## Desenvolvimento

```powershell
cargo fmt
cargo check
cargo test
```

## Troubleshooting rapido

- Nao encontrou certificado:
  - confira token conectado
  - confira middleware instalado
  - confira certificado no repositorio `MY` com chave privada
- Web app nao conecta:
  - valide `allowed_origins`
  - valide `ws_token`
  - valide host/porta/path (`ws_host`, `ws_port`, `ws_path`)
- Assinatura retornando `BUSY`:
  - ja existe assinatura em andamento (bandeja ou websocket)

## Observacoes

- O app usa o dialogo seguro do Windows/token para PIN quando necessario.
- O protocolo e local (localhost), sem TLS por padrao.
