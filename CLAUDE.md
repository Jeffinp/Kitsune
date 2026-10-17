# CLAUDE.md — Kitsune

Sistema operacional x86_64 escrito do zero em Rust (com C onde vale a pena), do bootloader ao desktop. Dono: Jeferson
(português do Brasil; direto e prático, sem enrolação). Repositório `Jeffinp/Kitsune`, branch `master`, push direto
(sem PRs). Responda e documente **em português**.

## O que existe hoje (resumo; detalhes em `docs/ROADMAP.md` e `CHANGELOG.md`)

- Boot BIOS e UEFI, desktop animado pt/en, scheduler preemptivo (250 Hz), OJFS v3 (journal, CoW), rede (virtio-net,
  NE2000, DHCP, DNS, TCP, **TLS 1.3 com 46 raízes**), contas e permissões, apps WASM com sandbox.
- Programas ELF em **ring 3** (`int 0x80`): Terminal `kterm`, `jsrun` (QuickJS), `mediadec` (VP9+Opus), `play`.
- Navegador: HTML/CSS (flex, grid, floats, SVG), **JavaScript das páginas** (um `jsrun --page` por aba, DOM em JS),
  `fetch`/XHR/cookies/CORS/keep-alive, Preact e Vue rodam, `<video>`/`<audio>` com MSE, bloqueador de anúncios
  nativo (Shields). **O embed do YouTube toca** (vídeos que o servidor aceita), a ~25 fps em 240p no QEMU TCG.
- Som: driver AC'97 + mixer. Placar de vídeo: Shift+I sobre o vídeo.
- Meta de longo prazo: servidores (`docs/ROADMAP.md` item 13: microVM, SMP, E/S, compat Linux).

## Estrutura

| Pasta | O que é |
|---|---|
| `kitsune_core/` | lib `no_std` + `forbid(unsafe_code)`: toda a lógica testável no host (fs, net, web, browser, mídia, hw, wm, gfx) |
| `kernel/` | bare metal: drivers, scheduler, compositor, `desktop/`, `fetch/`, `userproc/`, `netstack/`, `audio` |
| `os/` | builder que monta as imagens BIOS/UEFI (`kitsune-bios.img`, `kitsune-uefi.img`) |
| `user-apps/` | `abi` (chamadas, crate de host), `sdk`, `kterm`+`play`, `jsrun` (C+Rust+JS), `mediadec`, `demo`, `wasmhost` |
| `tools/` | `perf/run.sh` + `perf/scen/*.sh` (cenários QEMU), `w24-site.py` (sites de teste), `media/`, `js-*-check.js` |
| `docs/` | `ARCHITECTURE`, `ROADMAP`, `BUILDING`, `TESTING`, `design/*` (um por assunto), `audit/` |

Leia antes de mexer num assunto: `docs/design/code-structure.md` (onde colocar o quê), `tls-browser.md` (navegador,
JS, rede, vídeo, YouTube: §10–§18), `processos.md` (ring 3 e chamadas), `midia.md`, `audio.md`.

## Comandos

```bash
cargo test -j2 -p kitsune_core      # ~3430 testes, ~1 min (use -j2: a memória é pouca)
cargo lint-kernel                   # clippy -D warnings no alvo bare metal
cargo lint-host                     # clippy do core e do os
cargo fmt --all
node tools/js-dom-check.js user-apps/jsrun/src/bin/jsrun   # idem js-net-check.js e js-media-check.js
cargo build --release -p os         # gera a imagem (6 min frio, 2 min incremental)
# imagem: target/release/build/os/*/out/kitsune-bios.img
QEMU_MEM=256M bash tools/perf/run.sh <img> bios <saida> <segundos> tools/perf/scen/<cenario>.sh
python3 tools/w24-site.py 8079      # sites de teste (/modern /js /net /video /mse /ads /preact /vue /yt ...)
.\run.ps1                           # Windows: constrói e roda no QEMU com WHPX (+ som AC'97)
```

Build do kernel precisa de `clang` (para `jsrun` e `mediadec`). `rust-toolchain.toml` fixa o nightly: não mude sem
decisão explícita.

## Regras de código (obrigatórias)

- **Arquivos `.rs` com no máximo ~600 linhas.** Passou disso: divida em `foo.rs` + pasta `foo/` com submódulos
  (`use super::*`, `pub(crate)`, `pub use` no pai). Testes grandes podem passar; arquivo de código não.
- `kitsune_core` é `no_std` e `forbid(unsafe_code)`: nunca adicione `std`/`unsafe` ali.
- Todo `unsafe` leva `// SAFETY:` com a invariante real (o lint falha sem).
- Handlers de IRQ não alocam nem pegam lock. Dado externo (rede, disco, HTML, CSS, JS) nunca é confiável: `get()`,
  `checked_*`, `saturating_*`; o release do kernel não tem `overflow-checks`.
- Lógica pura vai para `kitsune_core` com teste que falha antes e passa depois; o kernel só tem a cola de hardware.
- Texto visível ao usuário: chave em `assets/i18n/{pt,en}.txt` + `t!("chave")`; `cargo test -p kitsune_core i18n`
  confere chaves e acentos.
- O kernel é soft-float (sem SSE): nada de `f32/f64` no kernel; C em anel 3 pode usar SSE2. Entre Rust e C só
  inteiros e ponteiros.
- Dependência nova no kernel: justifique (tamanho, `no_std`, licença) e rode `cargo deny check`.

## Fluxo de trabalho

1. Mudou comportamento? Prove: teste no host e, se tocou o kernel/navegador, um cenário no QEMU. Mudança de
   desempenho: número antes/depois (QEMU sem KVM só vale como proporção; o `[trace]` do guest é a medida confiável).
2. **Atualize a documentação junto** (docs em português): `CHANGELOG.md`, o `design/*.md` do assunto, `ROADMAP.md`,
   `TESTING.md` se criou cenário; contagem de testes no README/ARCHITECTURE quando mudar muito.
3. Commits em português, uma mudança por commit, corpo dizendo o porquê e como foi verificado. `Signed-off-by` é do
   dono (`git commit -s`); não adicione em nome dele. Antes de empurrar: testes + `lint-kernel` + `lint-host` + `fmt`.
4. Nunca declare "funciona" sem rodar. Relate o que NÃO foi provado (UEFI, WHPX, hardware real) com franqueza.
5. Para tarefas grandes, pode-se delegar a agentes em `git worktree` separados e mesclar depois; combine números de
   chamada de sistema e arquivos novos por assunto para evitar conflito.

## Armadilhas que já custaram horas

- **Nunca use `CARGO_TARGET_DIR` no build do `os`**: o `cargo install` do bootloader fica esperando o lock do cargo
  pai (0% de CPU, para sempre). Use a flag `--target-dir /dev/shm/x` ou o `target/` padrão.
- **Trust store**: `kitsune_core/data/trust-store.*` nunca leva raiz extra. Para testar atrás de proxy que reassina
  HTTPS: `EXTRA_PEM=raiz.pem bash tools/gen-trust-store.sh`, e `git checkout -- kitsune_core/data/` antes de commitar.
- **Disco**: `target/` passa de 9 GB e cenários QEMU deixam capturas grandes; limpe `/tmp` e `/dev/shm` ao terminar.
  Com o disco cheio os testes morrem com SIGKILL sem mensagem.
- Não use `pkill -f` (casa com o próprio shell); mate o PID. Servidores de teste: `setsid nohup python3 ... &`.
- O servidor de teste e a porta: se a porta já estiver em uso o servidor falha em silêncio (cada cenário tem `PORT`).
- Cenários que clicam usam coordenadas do BIOS 1280x720 (`lib.sh`: `goto`, `click`, `typestr`, `shot`).
- `screendump` do monitor só rende ~4 fps: para gravar vídeo use o VNC (`tools/media/vnc-record.py`, cenário W62).
- Merge de branches de agentes: conflitos comuns em `CHANGELOG.md`, `ROADMAP.md`, `README.md` (mantenha os dois
  lados), listas de `mod`/`use`, e nos bytes do protocolo página↔navegador (`wire.rs`: `M` é worker, `V`/`v` é mídia).

## Convenções do protocolo e das chamadas

- Chamadas `int 0x80` em uso: 0–9 base, 16–21 host WASM, 32–42 arquivos, 48–54 janela e processos, 56–57 fila da
  página, 60–65 áudio, 66 `getrandom`, 67 `cpu_ms`. A próxima livre é a 68; registre em `user-apps/abi` (com teste
  de pack/unpack) e em `docs/design/processos.md`.
- Mensagens página→navegador (primeiro byte): `D A N F X C Q G B W S H O L K M E Z V v` (ver `script/wire.rs`).
- Limites que importam: 3 s de CPU por tarefa de JS, 96 MiB de JS por página, scripts até 4 MiB (10 por página),
  HTML da página até 1 MiB, `mediadec` com 128 MiB de heap.

## Estado e próximos passos (ordem sugerida; ver `docs/ROADMAP.md`)

1. **Validar no WHPX do dono**: YouTube com o placar (Shift+I): fps, descartados, resolução escolhida. Aberto por
   `http://10.0.2.2:8479/yt?go=1&v=<id>` com `python tools/w24-site.py 8479` (o embed exige `Referer`).
2. Tirar parse e layout do documento da página da thread do compositor (último soluço na carga do YouTube).
3. Página `/watch` do YouTube (HTML > 1 MiB, módulo de 9,7 MB: pede cache de bytecode e mais memória).
4. SMP e depois o caminho de servidor (item 13); `WebSocket`, `<iframe>`, módulos ES, canvas.
5. Ainda sem prova: UEFI nas últimas mudanças (rode `tools/verify-boot.sh`), hardware real, favoritos gravam em
   `/home/.bookmarks` (conta logada não escreve ali; o certo é a pasta do usuário, como `~/.shields`).

## Limites éticos do projeto

O bloqueador e o placar são transparentes e desligáveis; telemetria, se existir, é opt-in, local primeiro, com painel
do que seria enviado. Não contornamos proteções antiabuso de sites (o YouTube pode recusar IPs de datacenter: isso
é decisão deles). Licença PolyForm Strict; contribuições cedem direitos ao Titular (`LICENSE-CONTRIBUTORS.md`).
