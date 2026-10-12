# Changelog

Uma seção por marco, do mais novo para o mais antigo. O Kitsune (antes OSjeff; veja a primeira
seção) não tem releases versionadas; as seções são marcos na `master`. As seções anteriores ao
renomeio mantêm o nome da época.

## 2026-10 — Arquivos grandes divididos

- Os arquivos de código acima de ~600 linhas foram divididos por responsabilidade, sem mudar comportamento (a regra de
  `docs/design/code-structure.md`): `foo.rs` fica com a documentação, os `mod` e os reexports, e a pasta `foo/` com os
  submódulos (`use super::*;`). Núcleo: `web/form`, `browser/addressbar`, `format/jpeg`, `ui/{raster,brand,anim,appart}`,
  `fs3/mount`, `apps/shell/sys`, `system/settings`, `network/dns`, `windowing/winman`. Kernel: `trace`, `sched`,
  `wasm/abi2`, `netd`, `fetch`, `fb/shapes`. Os `impl` grandes (`Browser`, `FormState`, `Fs3`, `WindowManager`, `Netd`,
  `Canvas`) viraram vários `impl`, um por assunto. O pai de cada um ficou com no máximo ~410 linhas.
- Depois do merge do `jsrun`: `kernel/userproc` (`syscall`, `host`, `files`, `queue`) e os testes `i18n/audit` (`scan`,
  `accents`) também foram divididos. Sobraram acima de 600 linhas só arquivos de testes.
## 2026-10 — JavaScript em anel 3 (`jsrun`, QuickJS)

- Novo programa **`jsrun`** (`run jsrun arquivo.js`, `-e 'código'`, entrada padrão, `--selftest`): o **QuickJS**
  2026-06-04 (C, MIT, sem modificações) compilado com clang para bare metal, sobre o libm do **musl** (só double) e
  uma libc mínima própria (`user-apps/jsrun/libc`). `console.log/info/debug/error/warn`, `print`, `performance.now()`,
  `await` no topo, fila de promises, erro não pego com pilha e status 1, promises rejeitadas sem tratador relatadas.
  Limites: 48 MiB de memória JS, ~3700 chamadas aninhadas (vira `RangeError`). Etapa 1 do item 11 do roadmap
  (JavaScript/YouTube).
- Kernel: **chamada 9** (`unix_ms`, UTC em milissegundos, `Date.now()`), pilha e heap por programa embutido
  (`jsrun`: 4 MiB de pilha, 64 MiB de heap; só para a imagem embutida), **SSE para o anel 3** (`CR4.OSFXSR`/`OSXMMEXCPT`).
- SDK: `_start` alinha a pilha a 16 bytes antes de chamar o `main` (código C com SSE dava `#GP` em `movaps`);
  `unix_time_ms()`.
- Correção: um processo recém-criado podia rodar antes de entrar na tabela (ou achar nela o dono anterior da vaga do
  fio) e morrer em silêncio; `spawn` e `reap` agora fazem isso numa seção crítica só.
- Cenário `tools/perf/scen/w51-jsrun.sh` (56/56 no teste embutido; `fib(27)` ~0,3 s sob TCG, ~20 ms nativo no
  hospedeiro). Créditos em `THIRD-PARTY.md`.

## 2026-10 — Navegador: páginas reais (CSS externo e layout moderno)

- **Folhas de estilo externas**: `<link rel=stylesheet>` e `@import` são buscados pelo `fetcher` (fila por página, até 12
  folhas e 1 MiB) e o layout é refeito a cada folha. O "continuar mesmo assim" de um certificado vale também para
  as imagens e folhas do site.
- **CSS**: `@media`, `@supports`, `@layer`, `var()`, `calc()`/`min()`/`max()`/`clamp()`, `vw`/`vh`, `light-dark()`,
  `color-mix()`, cores translúcidas sobre o fundo do ancestral, `;` dentro de `url()`; seletores `:not()`, `:is()`,
  `:where()`, `:first/last/only-child`, `:nth-*`, `:empty`, `:link`, `:checked`, `:disabled`; entidades do HTML 4.
- **Layout**: `flex` (linha e coluna, wrap, grow/shrink/basis, gap, justify, align, order, margens auto), `grid`
  (`fr`, `repeat`, `auto-fill`, `span`), fileiras de `inline-block`, `float`/`clear` com texto contornando, tabelas em
  quirks mode, caixas escondidas de propósito. `MAX_NODES` 20 000, `MAX_RULES` 6 000.
- **Correção de TLS**: o buffer de registro era de 16 KiB exatos e todo registro de tamanho máximo falhava, cortando páginas
  grandes (a Wikipédia vinha com 15 KB de 72 KB); agora 16 KiB + 256. Índice de regras por tag, classe e id.
- **`::before`/`::after`** (texto gerado e clearfix), **combinadores de irmão** (`+`, `~`) e um **rasterizador de SVG** próprio
  (`format::svg`) para logos e ícones.
- **`grid-template-areas`**, `grid-area` e `grid-template: linhas / colunas` (a Wikipédia usa).
- Planejado: JavaScript e YouTube em etapas (`docs/ROADMAP.md`, item 11); bloqueador de anúncios nativo, escolhido pela pessoa (`docs/ROADMAP.md`, item 10).
- Ferramentas: `tools/render-page.py` + exemplo `render_page` (renderizam páginas reais no host),
  `tools/fetch-page-css.py`; cenários `w49-browser-css.sh` e `w50-real-site.sh`.

## 2026-10 — O Terminal em anel 3 (`kterm`)

- Novo programa **`kterm`** (`run kterm`): o motor de shell do `kitsune_core` (`Term`, `Shell`, `ShellFs`, `SysInfo`) num
  processo de anel 3 com janela própria. Arquivos pelas chamadas 32–42 (direitos do usuário: gravar na raiz dá
  "Permissão negada"), `run` com filhos, pipe, argumentos e `Ctrl+C`, `date`, título com a pasta atual. Cenário
  `w48-ring3-terminal.sh`.
- Chamadas novas: **`spawn`, `proc_poll`, `proc_kill`** (52–54: um processo inicia e acompanha filhos com a `Cred` do
  pai; o kernel guarda o filho até o pai coletar o fim), `time` (7) e `language` (8). Contratos `pack_spawn`/`pack_poll` no
  `kitsune-userabi` (testados).
- SDK: recurso `heap` (alocador por `brk`) e `local_time_secs`/`language`; a tabela da fonte 8x8 passou a um só arquivo
  (`user-apps/glyphs.rs`) incluído pelo kernel e pelo `kterm`. Pilha maior (512 KiB) para programas que rodam o shell.

## 2026-10 — Janela e entrada para processos em anel 3

- Um programa ELF pode ter **uma janela** (`win_open`, `win_present`, `win_event`, `win_title`; chamadas 48–51): ele
  desenha em memória própria (RGBA) e entrega o quadro; recebe teclas, ponteiro, novo tamanho e o pedido de fechar.
  Para o desktop é um app como os outros: um *slot sem runtime WebAssembly* na tabela do gerenciador
  (`wasm/manager/native.rs`), então compositor, título, minimizar, redimensionar, Tarefas e fechar funcionam sem
  mudança; os quadros e a conversão para o formato da tela passam pela thread `procfsd`, com a trava só por faixa.
  Fechar a janela avisa o programa e depois o encerra; o processo que acaba deixa a janela com o motivo.
- SDK `kitsune_user::win`; programa `gfxdemo`; `Event` e as chamadas no crate `kitsune-userabi` (testados). Cenário
  `w47-ring3-window.sh` (quadros, ponteiro, tecla, fechar, `Ctrl+C`).

## 2026-10 — Arquivos, argumentos e pipe para processos em anel 3

- Programas ELF ganham **arquivos**: `open`, `close`, `read`, `write`, `seek`, `stat`, `readdir`, `mkdir`,
  `remove` (para a lixeira), `rename` e `getargs` (chamadas 32–42), com os **direitos de quem os iniciou**
  (`Secured` + `Cred` guardada no `run`: `EACCES` quando o dono, o grupo e o modo não deixam). As chamadas só
  enfileiram o pedido; a thread `procfsd` faz o disco em contexto de thread e o processo é reiniciado para pegar o
  resultado. `run PROGRAMA [ARGS...]` passa argumentos, a pasta do terminal e o pipe (`echo oi | run cat`).
- Novo crate sem dependências `user-apps/abi` (`kitsune-userabi`): números, flags, `errno`, `StatRec`,
  `resolve` de caminhos, `pack_args`/`unpack_args`; testado no host, fuzz `userabi`. SDK: `kitsune_user::fs`;
  programas `cat`, `ls`, `fstest`; `print!` agora faz uma só chamada por linha.
- Cenário `w46-ring3-files.sh` (22 passos do `fstest` + `cat`/`ls`, inclusive a recusa de gravar em `/etc/accounts`).

## 2026-10 — Desempenho: disco, boot e catálogo

Medido com `-icount shift=0` (repetível; ver `docs/BENCHMARKS.md`), primeiro boot em disco vazio e boot seguinte:

- **Disco (ATA PIO)**: a transferência cedia a CPU após *cada setor*, então cada setor esperava a vez de todas
  as outras threads (uma escrita de 16 setores levava ~24 ms com o compositor ocupado). Agora cede a cada 32
  setores. Formatar o disco no primeiro boot: 2,53 s → 0,18 s; contas e pastas: 4,97 s → 0,44 s; a primeira
  leitura de diretório de um app: 716 Mciclos → 9,6.
- **Catálogo de apps**: o launcher lia cada pacote *inteiro* (o `paint` tem 590 KB) só para ler manifesto e
  ícone, e a atualização de pacotes embutidos lia todos de novo a cada boot. `appinstall::read_package_meta`
  anda pela tabela de seções (mesmas checagens do `wasmsec`) e lê só os cabeçalhos mais as seções do
  manifesto e do ícone. Boot seguinte: 377 ms → 10 ms nessa etapa. Testado contra a leitura completa
  (válidos, truncados, magia/versão/id ruins, duplicados, nomes antigos) e que o código nunca é lido.
- **Senha**: `pbkdf2` guarda os estados do hash após `ipad` e `opad` (2 compressões por iteração em vez
  de 4): 291 → 146 Mciclos por derivação (login, troca de senha). Mesmos vetores RFC e teste contra o laço HMAC simples.
- **Papel de parede**: `Canvas::glow` só percorre a corda do círculo em cada linha, sem divisão nem teste
  por pixel (-24 %; pixels idênticos: 0 de diferença contra a captura anterior).
- **Quadros de animação**: os quadros eram gerados a cada tick (até 250 por segundo) enquanto qualquer coisa
  animasse, e o pisca do cursor do Terminal e do Editor mantém a tela animando por 12 s após cada tecla. Agora
  `Desktop::animation()` separa *movimento* (arrastar, abrir/fechar, apps ao vivo, rolagem: no máximo 125
  quadros/s) de *ambiente* (só o cursor piscando: ~31/s), e o pisca repinta só a célula do cursor (o `Layer`
  já aceitava um retângulo sujo), pulando as linhas fora do recorte. Digitar e esperar: 51 % de CPU → ~3 %
  (quadro de 6,4 ms → 0,9 ms, 120 → 31 por segundo). As animações são por tempo real, então o movimento é o
  mesmo. O modo de verificação (Ctrl+Alt+V) só aponta a própria célula do cursor, igual à versão anterior
  (o alfa do pisca depende da hora em que se pinta).
- **Boot**: o desktop (`Desktop::new`, configurações, papel de parede) passou a ser montado *durante* a vinheta,
  um passo entre dois quadros, em vez de depois dela (a vinheta dura ao menos 5 s de qualquer jeito, e o
  trabalho se somava). O fundo da vinheta (gradiente + brilhos, a parte cara) é desenhado uma vez e copiado a
  cada quadro; a cor de destaque é fixada no começo, então carregar as configurações não a troca no meio.
  Primeiro quadro do desktop (disco já formatado, TCG): ~6,5 s → ~5,0 s, agora limitado pelos 5 s da vinheta.
- **Vinheta**: agora termina quando o desktop está pronto (mínimo de 1,5 s, em ticks e não em segundos inteiros do
  RTC); a barra segue o relógio, espera em 80 % se o trabalho atrasa e completa ao fim. Primeiro quadro do
  desktop em disco novo: 8,8 s → 3,5 s (BIOS) e 8,8 s → 4,0 s (UEFI); disco já formatado, medido no TCG:
  ~2,6 s.
- Ferramentas: `bench ata`, `bench password`, `bench app` no build `perf-trace`, marcas dentro de
  `Desktop::new`, tempos do papel de parede.

## 2026-10 — Apps WebAssembly em anel 3

- Os apps **v2** (`osj.*`) rodam num **processo de anel 3 por app**: `user-apps/wasmhost` carrega o
  `wasmi`, a memória do app e o combustível; cada import vira uma chamada de sistema (16–20) que o kernel
  atende. O contrato (`kitsune_core::appabi::remote`) é testado no host. As funções `osj.*` passaram a ser
  **uma só implementação** (`abi2::call` sobre um `Guest`), usada pelo wasmi do kernel e pelo processo.
  Chamadas bloqueantes reiniciam ao acordar. Se não houver vaga de processo, o app roda no kernel como antes.
  Verificado em QEMU (cenário W44): Notas, Pintura e Relógio, `kill` e fechamento de janela encerram só o
  processo e devolvem os quadros. `linker.ld` agora põe `.got` na página de dados. Detalhes em
  [`docs/design/processos.md`](docs/design/processos.md).
- **Desempenho dos apps em anel 3**: de ~8,6× para ~1,2× o custo do app no kernel (passagem direta de CPU
  entre processo e `appd`, lote de chamadas sem resultado, `get_size`/`lang` dentro do `Job`, thread de
  kernel sem recarregar o CR3, hospedeiro em `opt-level 3`). Medição: `w45-wasm-cost.sh`. Corrigido um
  #PF só do build `perf-trace` (a amostra do timer lia o código de um processo de usuário; SMAP).

## 2026-10 — Processos em anel 3 (MVP)

- Programas **ELF rodam em modo usuário**, cada um no seu espaço de endereçamento: GDT com segmentos de
  usuário e `TSS.RSP0` por thread, alocador de quadros físicos (`paging::FrameAlloc`), tabelas de páginas
  por processo (metade do kernel sem bit `USER`; a do usuário começa vazia), carregador ELF estrito
  (`format::elf`: W^X, sem páginas divididas, limites), `int 0x80` com ponteiros sempre traduzidos pelas
  tabelas do processo, processos recolhidos e vagas de thread reaproveitadas. SDK `user-apps/` e comando
  `run` no Terminal. Verificado em QEMU: escrita no endereço 0 e instrução privilegiada matam só o
  processo (`#PF`, `#GP`), um laço infinito é preemptado e encerrado com Ctrl+C, 20 falhas seguidas não
  vazam um quadro; SMEP e SMAP ligados quando a CPU os tem; chamada `brk` (heap de até 8 MiB, quadros devolvidos ao fim). Fuzz `elf_frames`. Detalhes e limites em [`docs/design/processos.md`](docs/design/processos.md).

## 2026-10 — Contas e permissões (base), plano de segurança e compatibilidade

- Novo grupo `security` no `kitsune_core`: contas (`account`: usuários, grupos, banco de dados em texto
  validado), senhas com **PBKDF2-HMAC-SHA-256** e sal (`password`, vetores do RFC 4231 e do PBKDF2),
  regras de permissão `rwx` com *sticky* (`perm`) e limitação de tentativas de login e bloqueio por
  inatividade (`session`).
- OJFS v3 guarda o **grupo** no inode (campo que estava sempre zerado: discos antigos continuam
  válidos, com grupo 0); o VFS ganhou `set_owner` (chmod/chown) e `stat` devolve dono, grupo e modo.
- `storage::secured::Secured`: adaptador do VFS que impõe as permissões de um usuário (busca nas pastas
  acima, leitura, escrita, criação com dono e `umask`, remoção e renomeio com *sticky*, lixeira por
  usuário). 16 testes no host; fuzz `security_db`.
- **GIF** (primeiro quadro, LZW, paleta local e global, entrelaçado, transparência) e **JPEG** (*baseline*
  e sequencial estendido de 8 bits: cinza e YCbCr, subamostragens 4:4:4, 4:2:2, 4:4:0 e 4:2:0, intervalos de
  reinício, tabelas de 16 bits) no navegador, no Imagens e no papel de parede. O JPEG foi conferido contra o
  libjpeg-turbo em 12 imagens (diferença máxima de 3 níveis por canal); progressivo, CMYK e 12 bits dizem
  "não suportado" em vez de mostrar uma imagem errada. Fuzz `image_decode` ganhou os modos GIF e JPEG.
- **Contas no kernel** (primeira etapa): no primeiro boot cria `/etc/accounts` com o administrador `kitsune`
  sem senha, que entra sozinho (a área de trabalho abre como antes) e **adota** tudo o que já estava no disco
  (`/Documentos`, `/Imagens`, arquivos soltos e o `/home` antigo vão para `/home/kitsune`; o topo do disco
  fica só com as pastas do sistema, do `root`). Todo o acesso do usuário aos arquivos (Arquivos, Editor,
  Terminal, Imagens) passa por `Secured` com as permissões de quem entrou; configurações, registros, apps e
  a base de contas usam o acesso do sistema. Os lugares do Arquivos (Início, Documentos, Imagens) e a pasta
  inicial do Terminal e dos apps `fs=home` seguem o usuário. Terminal: `whoami`, `id`, `groups`, `users`,
  `chmod` (octal e simbólico, `-R`), `chown`, `chgrp`, `ls -l` e `stat` com dono, grupo e modo. Verificado em
  QEMU: um `chmod 000` impede o `cat`, e criar algo em `/` é recusado.
- **Login e página Usuários.** Com senha definida ou mais de uma conta, o boot mostra a **tela de login**
  (avatares das contas, Tab troca, Enter entra; erro genérico, espera crescente após 3 erros, tudo no
  `LoginGuard`). Bloquear a tela (Ctrl+Alt+L ou menu do sistema) e **Sair** voltam a ela. Ajustes ganhou a
  página **Usuários**: sua conta e troca de senha (pede a atual), lista de contas, definir senha, tornar
  administrador ou comum e remover (com confirmação) para administradores, e formulário de nova conta.
  Verificado em QEMU (W38/W39): criar `maria` com senha, reiniciar, senha errada recusada, senha certa
  abre `/home/maria`; o PBKDF2 (20 000 iterações) leva ~140 ms em QEMU/TCG. Registro de auditoria
  (`auth: maria: signed in`, `sign-in refused (wrong password)`, mudanças de conta e de senha; nunca a senha)
  no Registro e na serial. A senha digitada (login e Ajustes) vive num `Secret` (`security::secret`) de
  capacidade fixa, sem realocar, zerado ao limpar ou soltar e que não se imprime. **Bloqueio automático** da tela depois de 5, 15 ou 30 minutos sem teclado nem
  mouse (Ajustes > Usuários; só para quem tem senha), registrado como `auth: screen locked ...`; verificado em QEMU.
- **JPEG progressivo** (seleção espectral e aproximação sucessiva, com intervalos de reinício): decodifica
  exatamente os mesmos pixels que o arquivo sequencial feito da mesma imagem (testado em 4:2:0, 4:2:2, 4:4:4,
  cinza e com reinício) e confere com o libjpeg onde a ampliação da cor é exata; um arquivo cortado mostra o
  que chegou (a primeira varredura já dá a imagem, borrada). Limite de ~6 milhões de amostras, porque os
  coeficientes ficam na memória; acima disso o Imagens e o navegador dizem que é grande demais.
- **Formulários no navegador:** `method=post` (corpo `application/x-www-form-urlencoded` com
  `Content-Length`; um *redirect* 301/302/303 vira GET, 307/308 mantêm o corpo; recarregar ou voltar
  pede o endereço com GET para não repetir um envio sem querer), `<select>` (clique ou setas/letra
  trocam a opção; a lista suspensa ainda não existe) e `<textarea>` (várias linhas com quebra
  automática, Enter insere quebra, ↑/↓/Home/End por linha, limite de 4 KiB). O parser passou a
  fechar `<option>` implicitamente. `tools/nettest-server.py` ganhou `/form`, `/echo` e `/redir-post`;
  cenário W40 verificado em QEMU (envio, eco e POST com 303).
- **ZIP, gzip e codificações de texto.** Novo `format::zip` (listar, extrair e criar com `stored`/`deflate`,
  CRC-32 conferido, o tamanho declarado é teto duro contra "zip bomb", nomes com `..`, `\`, unidade ou
  caracteres de controle são recusados) e `format::text` (UTF-8 com BOM, UTF-16 LE/BE com marca,
  Windows-1252/Latin-1). Terminal: `zip [-r]`, `unzip [-l -o -d]`, `gzip`/`gunzip [-d -k -c -f]` e
  `iconv -f -t`. Arquivos: "Extrair aqui" (numa pasta nova, nunca por cima do que existe) e "Compactar em
  ZIP". O Editor abre arquivos Windows-1252 e UTF-16 como texto, mostra a codificação na barra de estado e
  grava de volta do mesmo jeito (se o texto ganhar um caractere que ela não escreve, vira UTF-8). Fuzz
  `archive_text`; verificado em QEMU (W41).
- **SDK documentado:** [`docs/SDK.md`](docs/SDK.md), "seu primeiro app em dez minutos": criar a crate, escrever
  o app (com arquivo e idioma), compilar, baixar o `.wasm` com `wget` no Terminal e abri-lo com Enter no
  Arquivos (instala e executa), sem recompilar o sistema. O texto foi seguido de ponta a ponta em QEMU
  (W42). `fs=home` agora é a pasta de quem entrou.
- (histórico) O kernel não usava essas peças ainda (não havia login): o plano em fases está em
  [`docs/design/usuarios-seguranca.md`](docs/design/usuarios-seguranca.md) e o de compatibilidade em
  [`docs/design/compatibilidade.md`](docs/design/compatibilidade.md); o roadmap foi atualizado.

## 2026-10 — Hardware, dívida técnica

- **Vídeo:** `tools/gpu-matrix.sh` inicia a imagem em cada adaptador de vídeo que o QEMU emula (BIOS e
  UEFI): 11 de 12 casos chegam ao desktop. O que falha é `virtio-gpu-pci` sem VGA em UEFI (a GOP é só de
  *blit*, sem framebuffer linear). Detalhes em [`docs/HARDWARE.md`](docs/HARDWARE.md). O boot passa a
  registrar o framebuffer (resolução, stride, formato) e `tools/hw-check.ps1` repete a matriz no Windows
  e prepara o pendrive.
- **Notas 1.1.0** aceita acentos e qualquer caractere UTF-8 (cursor, apagar e quebra de linha por
  caractere). As Propriedades de um app mostram o nome no idioma em uso.
- Um app embutido instalado numa versão **anterior** é atualizado no boot (relógio, pintura e cobrinha
  1.0.1, notas 1.1.0), sem ressuscitar um app que o usuário removeu.
- `bench/` volta a compilar (módulos de `fb` divididos, API atual do shell e do editor).
- Divisão de arquivos longos do kernel: `main.rs` (737 → 503 linhas, o resto em `bringup.rs`),
  `fetch/`, `netstack/` e `wasm/manager/` em módulos por responsabilidade; comportamento verificado por
  boot em BIOS e UEFI, apps e `curl`.
- Divisão de arquivos longos do `kitsune_core` (só movimentos de código, 2958 testes passam): layout e
  estilo da web, navegador, builtins e executor do shell, linha de comando e parser, gerenciador de
  arquivos, editor, VFS, fs3, inflate, PNG, imagem, fonte TTF, atividade, klog e DHCP. Restam poucos
  arquivos entre 600 e 970 linhas (o maior é a auditoria de i18n).

## 2026-10 — Disco: limite de tempo e nova tentativa

- O driver ATA desistia por **contagem de leituras do registrador de estado**: numa máquina carregada
  (a thread de E/S do emulador sem CPU) a CPU lia rápido demais e declarava o disco morto, e o volume
  ficava somente leitura (`ata: transfer failed (Timeout)`). Agora uma espera só se esgota depois de
  **1 milhão de leituras e de 3 s de tempo real** (limite duro de 40 milhões se o relógio não anda), e um
  controlador declarado morto recebe **uma nova tentativa a cada 10 s** em vez de ficar morto para sempre.
  A política é pura e testada (`kitsune_core::hw::ata::{poll_exhausted, dead_retry_due}`).

## 2026-10 — Dependências e edição 2024 nos apps

- **Edição 2024 do Rust em todos os crates**: o SDK e os apps (`wasm-apps/*`, `examples/*`) estavam na
  2021. As macros do SDK passaram a gerar `#[unsafe(no_mangle)]` e `#[unsafe(link_section = ...)]`; a
  Cobrinha deixou de tomar referências compartilhadas a `static mut` (`&raw const`). Os quatro apps
  embutidos e os três exemplos compilam sem avisos.
- **Dependências**: `wasmi` 1 → 2 (a validação dos módulos passa a ser pedida explicitamente com a
  feature `validate`; o runtime precisa de mais pilha: a thread `appd` roda numa pilha de 512 KiB, via
  `sched::spawn_with_stack`), `smoltcp` 0.12 → 0.14, `rustls-webpki` 0.103.13 → 0.103.15,
  `rustls-pki-types` 1.14.1 → 1.15.1 e as atualizações compatíveis do `Cargo.lock` (workspace, fuzz e
  apps). Combustível e limite de memória do runtime novo foram conferidos com um teste isolado
  (laço infinito → `OutOfFuel`; `memory.grow` acima do limite → -1).
- **Não atualizadas, de propósito**: `sha2` 0.11, `p256`/`p384` 0.14 e `rsa`: o `rsa` 0.9 ainda depende
  do `digest` 0.10 e não compila com `sha2` 0.11; ficam juntos em 0.10/0.13/0.9 até haver um `rsa`
  estável para o `digest` 0.11. O `rsa` continua com a exceção do aviso Marvin documentada em
  `deny.toml`. O nightly fixado (`nightly-2026-10-05`) também fica: trocar de nightly é uma decisão
  deliberada, com verificação própria.
- Verificado: 2953 testes, clippy, `cargo deny`, `cargo audit`, `cargo fuzz build`, boot BIOS e UEFI, os
  quatro apps abrindo e rodando, HTTP simples e uma conexão TLS (certificado autoassinado recusado).

## 2026-10 — README e documentação falam só do Kitsune (W35)

- **README.md e README.en.md reescritos:** apresentação, destaques com números, capturas, como começar (Windows
  nativo e WSL, flags do `run.ps1`), o que tem dentro, como funciona, qualidade, segurança, estado do projeto,
  índice da documentação, estrutura e licença.
- **`docs/BENCHMARKS.md` (novo):** cada número citado nos READMEs, com o comando que o reproduz, a data (2026-10-10) e
  o aviso de que são proporções de QEMU sem KVM (tempo de boot, tamanho da imagem, heap ocioso, quadros, testes).
- **Interface:** `docs/design/ui-identity.md` agora registra só as decisões do shell do Kitsune e `ui-macos.md`
  virou `docs/design/ui-design.md` (tokens, toolkit, regras de desenho, orçamento de desempenho, seções por app);
  sem tabelas de comparação nem notas de pesquisa.
- **Neutralidade:** comentários e documentos passam a usar só os nomes do Kitsune (Tarefas, Busca, Apps, painel,
  barra de tarefas); `Icon::Launchpad` virou `Icon::Apps` e a curva `GENIE` virou `SWOOP`. Instruções de compilação
  do host (Windows, WSL, `apt`) ficam como requisitos do host em `docs/BUILDING.md`.
- **Roadmap e segurança:** `docs/ROADMAP.md` reescrito (entregue / próximos passos com critério de aceite);
  `SECURITY.md` e `docs/SECURITY-MODEL.md` corrigidos (linhas duplicadas e valores antigos removidos, sem tirar
  nenhuma ressalva). Contagens atualizadas: 2953 testes, 17 alvos de fuzz.

## 2026-10 — Código organizado por responsabilidade (W34)

- **Refatoração pura, sem mudança de comportamento** (telas do boot idênticas pixel a pixel à linha de base).
  `kernel/src/desktop/` (~40 arquivos soltos) virou `windows/ shell/ input/ kit/ services/ apps/<app>/ compositor/`,
  cada app com seu estado, entrada e desenho numa pasta; nenhum arquivo passa de ~650 linhas.
  `kitsune_core/src` (~100 módulos soltos) virou `ui/ storage/ network/ browsing/ format/ platform/ system/
  windowing/ apps/` ao lado de `hw/` e `i18n/`; `lib.rs` reexporta os nomes antigos (`kitsune_core::fs3`).
  Um teste de estrutura confere o grafo de dependências entre as pastas. Os testes de cada módulo ficam ao lado
  dele (`tests.rs` / `tests/` por assunto). Mapa, regras e checklists em `docs/design/code-structure.md`.
- **Apps de demonstração fora da imagem:** `plasma` e `hello` (Olá) deixam de vir embutidos e `nettest` (Teste de
  rede) segue como fixture; os três ficam em `wasm-apps/examples/` e continuam compiláveis. Vêm na imagem:
  Cobrinha, Notas, Pintura e Relógio.

## 2026-10 — O sistema passa a se chamar Kitsune (W33)

- **Nome.** OSjeff vira **Kitsune** (a raposa de nove caudas do folclore japonês). Detalhes do nome em
  `docs/brand/NAMING.md`.
- **Marca.** Uma raposa geométrica de frente, vetorial e procedural (`kitsune_core::brand`: 12 polígonos, versões
  ícone, silhueta de uma cor e com caudas, redesenhada e ajustada para 16 a 128 px). Ela aparece no botão Apps do
  painel, na barra de tarefas, em Ajustes > Sobre, na vinheta de abertura (com fade), nas abas das páginas
  `kitsune://` e como ícone de app sem ícone. Os SVG, a paleta e as regras de uso estão em `docs/brand/`.
- **Renomeado:** o crate `osjeff_core` virou `kitsune_core` (diretório incluso), as imagens `osjeff-bios.img` e
  `osjeff-uefi.img` viram `kitsune-bios.img` e `kitsune-uefi.img`, e os scripts, o CI, o fuzzing, os benchmarks e o SDK dos
  apps acompanham. Todo texto visível, o `User-Agent` (`Kitsune/<versão>`), o nome de máquina padrão (`kitsune`) e os
  catálogos de idioma usam o nome novo; a tela de falha diz `KITSUNE - KERNEL PANIC`. As variáveis de ambiente de
  desenvolvimento passam a `KITSUNE_*` (`KITSUNE_STORAGE_SELFTEST`; a antiga `OSJ_STORAGE_SELFTEST` ainda vale).
- **Protocolos e formatos:** `osjeff://` vira `kitsune://`; `/etc/osjeff.conf` vira `/etc/kitsune.conf`; as seções
  WASM `osjeff.manifest` e `osjeff.icon` viram `kitsune.manifest` e `kitsune.icon`.
- **Compatibilidade mantida:** o navegador ainda entende `osjeff://` (e mostra o endereço novo); o arquivo de
  configuração antigo é lido quando o novo não existe e some no primeiro salvamento; pacotes de apps com as seções
  antigas continuam instalando; o módulo de importação `osj` dos apps não muda (abreviação histórica).
- **Disco:** o formato em disco (`OJF2`, `OJF3`) não mudou, discos antigos montam como antes (teste com imagens
  geradas pelo formatador anterior). O nome do formato, OJFS, é histórico e pode mudar numa revisão futura.


## 2026-10 — Terminal, Tarefas, Registro e Calculadora em dois idiomas (W31)

- Todo texto visível do **Terminal** (mensagens, uso e `help` de cada comando, títulos de `df`/`free`/`ps`/`ping`/
  `ifconfig`, banner), de **Tarefas**, do **Registro** e da **Calculadora** vem do catálogo, com acentos em
  português e texto natural em inglês (blocos `sh.*`, `term.*`, `tasks.*`, `log.*`, `calc.*`). As mensagens do
  interpretador seguem o idioma **no momento da execução**; nomes de comandos, opções, status de saída e tudo que
  um script pode ler ficam iguais nos dois idiomas (`shell::tests::i18n` roda o mesmo script nos dois).
- Números, tamanhos, tempos e plurais usam os formatadores do idioma (`1,5 KiB` | `1.5 KiB`, `1 processo` | `1 process`);
  a Calculadora usa a vírgula ou o ponto decimal do idioma.
- Testes de idioma por thread (`i18n::testlang`), sem disputa entre testes em paralelo; cenários `w31-term.sh` e
  `w31-apps.sh` no QEMU e quatro telas lado a lado em `docs/img/i18n-w31-*.png`.
## 2026-10 — Navegador, Ajustes, apps e componentes nos dois idiomas (W32)

- **Navegador**: barra de endereço, abas, página inicial, busca, menu de contexto, avisos, indicador de segurança
  e as páginas de erro (sem conexão, site não encontrado, conexão recusada, tempo esgotado, cada problema de
  certificado, redirecionamentos) saem do catálogo e seguem a troca de idioma na hora; `osjeff://favoritos`,
  `historico` e `sobre` são geradas de novo no idioma novo (`<html lang>` acompanha). O pedido HTTP diz o
  idioma da interface em `Accept-Language` (`pt-BR,pt;q=0.9,en;q=0.8` ou `en;q=1`).
- **Ajustes**: todas as páginas, mensagens, nomes de papéis de parede, cores e das 52 cidades do fuso (`Lisboa`/
  `Lisbon`); a busca de cidade acha pelos dois nomes; data e hora pelos formatadores do idioma.
- **Apps WASM**: o manifesto aceita `name.pt=`/`name.en=` (com acentos); os apps que acompanham o sistema
  viram Relógio, Notas, Pintura, Olá, Cobrinha e Teste de rede; erros de instalação e remoção, o motivo de um app
  ter encerrado e a recusa de um arquivo que não é app (aviso na tela) estão nos dois idiomas. Novo `osj.lang()`,
  texto UTF-8 em `draw_text` e `tr(pt, en)` no SDK: os apps escrevem com acentos e acompanham o idioma.
- **Componentes e avisos**: a galeria sai do catálogo (e a amostra monoespaçada não cita linguagem de
  programação); `notify::notify_key` mostra um aviso no idioma da interface e grava o texto em inglês no log.
- Cenários de QEMU `w32-settings|web|apps|install|toast.sh` e quatro capturas lado a lado em `docs/img/i18n-w32-*`.

## 2026-10 — Um compositor correto por construção (W27)

- O desktop piscava com várias janelas abertas: sombras sumiam (a do Editor), o Snake piscava e uma
  janela era pintada acima de outra que está acima dela. Causa: vários caminhos de desenho que
  duplicavam uns aos outros e uma assinatura de cache que cada recurso novo precisava estender.
- **Um caminho só.** O desktop é descrito como uma cena de camadas (janelas com a sombra no footprint,
  barra, painel, overlays); o motor de dano (`osjeff_core::compositor`: `Region` de retângulos disjuntos,
  `Scene`, `Engine`) compara com o quadro anterior e planeja o que repintar; cada camada é pintada de
  baixo para cima sobre o dano, menos o que uma camada opaca esconde; só o dano sobe à VRAM. Arrastar,
  o relógio e o gráfico que desliza são casos da mesma regra. Cursor, toasts e HUD continuam só no
  framebuffer (cursor apagado primeiro e pintado por último).
- **Provado.** Teste diferencial no host (milhares de históricos aleatórios: o resultado incremental é
  idêntico ao redesenho completo depois de cada quadro) com 11 testes de mutação permanentes e o alvo de
  fuzz `compositor_ops`; no QEMU, o oráculo `w27-oracle.sh` (tela em repouso contra o modo referência,
  Ctrl+Alt+R) e o modo verify (Ctrl+Alt+V). Antes: janelas fora de ordem e sombras faltando; depois: 0
  pixels de diferença (BIOS e UEFI).
- Removidos o cache `STATIC`, `render`, `render_anim_frame`, `anim_signature`, `WindowManager::signature`,
  `wm::scene_signature`. O painel é uma camada opaca acima das janelas. Arrastar ficou 35 % mais barato;
  Snake + Tarefas, de 13 para 5 ms por quadro. Projeto: `docs/design/compositor.md`.

## 2026-10 — Idiomas e acentos (W28)

- **Português do Brasil e inglês**, trocados ao vivo em *Ajustes > Idioma e região* (nova seção, cada opção no
  próprio idioma, formato da hora "pelo idioma / 24 h / 12 h" e uma dica, sem troca forçada, para usar o teclado
  ABNT2). `language=` e `clock=auto` entram no arquivo de configurações; arquivos antigos continuam valendo.
- `osjeff_core::i18n`: catálogos `chave = valor` (`assets/i18n/`) compilados por uma `const fn`, consulta com
  reserva (idioma, inglês, a chave) sem alocar, marcadores `{nome}` tipados, plurais (`pt`: 0 e 1 no singular),
  números, tamanhos, datas e horas por idioma, macros `t!`/`tp!`/`tk!`. Um idioma novo é só um arquivo de texto.
- Painel, menus, configurações rápidas, calendário, barra de apps, Apps, Busca, folha de energia, banners e a
  lateral de Ajustes saem do catálogo. A tela de falha e os logs ficam em inglês.
- Testes que acusam chave faltando ou sobrando, marcador diferente, plural incompleto, **palavra sem acento**
  (lista de ~190) e glifo ausente nas fontes; alvo de fuzz `i18n_format`; `tools/i18n-audit.py` e
  `docs/design/i18n-audit.md` listam o que falta migrar, por app.

## 2026-10 — Os apps do sistema (W25)

- **Tarefas** reúne o Gerenciador de tarefas e o Monitor de recursos num monitor de atividade com
  abas **CPU | Memória | Disco | Rede | Processos**: uso total e gráfico de 60 s que desliza a cada
  amostra, barras por processo, carga média, medidor de pressão da memória, memória por app, leitura e
  gravação do disco (contadores novos no driver ATA, sem lock e sem alocação), arquivos e pastas, IP,
  roteador, DNS, concessão e tráfego da rede, e uma tabela de processos com nomes amigáveis (Interface,
  Rede (busca), Aplicativos, Terminal (execução), Registro, Sistema), colunas ordenáveis, busca,
  Reiniciar e Encerrar (um serviço do sistema pede confirmação). Valor ao passar o mouse sobre os
  gráficos; números em português, alinhados; atualização suave e custo zero com a janela oculta.
  O tipo `Monitor` saiu; a Busca ainda o encontra ("monitor", "memória", "disco", "rede").
- **Registro**: tabela com hora, etiqueta colorida do nível, origem e mensagem em fonte mono; busca,
  filtro de nível, chave Seguir, Limpar e Salvar; rolagem suave com barra que some.
- **Ajustes** (eram as Configurações): barra lateral com Aparência, Papel de parede, Barra de apps,
  Teclado, Data e hora, Rede, Disco, Energia e Sobre; chaves, controles deslizantes, amostras de cor com
  anel, miniaturas ao vivo do papel de parede, lista de 52 cidades com busca, ampliação da barra de apps
  e duração dos banners (campos novos no arquivo, leitor ainda total).
- **Calculadora**: teclado arredondado com o destaque na coluna dos operadores, visor grande que encolhe,
  faixa de histórico, copiar, porcentagem, troca de sinal e memória; o teclado funciona.
- **Notificações**: ícone por nível, botão de fechar ao passar o mouse e linha que mostra o tempo que falta.
- Núcleo: `osjeff_core::activity` (nomes, formatação pt-BR, suavização, taxas de contadores que dão a
  volta, carga, pressão, tabela ordenável) e dez glifos novos; `FixedBuf` passou a imprimir UTF-8.
  Testes: 2532 no `osjeff_core` (eram 2497). Detalhes e números em `docs/design/ui-macos.md` (seção 11),
  `docs/design/sysmgmt.md` e `docs/TESTING.md`.
## 2026-10 — Identidade própria do shell (W26)

- **Janelas sem cara de macOS.** Botões de minimizar, maximizar/restaurar e fechar à **direita**,
  planos (células de 40x32, vermelho ao passar no fechar, apagados na janela sem foco), ícone do
  app e título alinhado à esquerda, botão de menu na barra de título com as mesmas entradas de
  Arquivo, Editar e Visualizar, e um fio de destaque na borda de cima da janela em foco. Pesquisa e
  decisões em `docs/design/ui-identity.md`.
- **Encaixe de janelas.** Arrastar à borda de cima maximiza, às laterais encaixa a metade, aos
  cantos o quarto, com um contorno animado; `Alt+setas` (e `Alt+Shift+setas`) encaixam, maximizam,
  restauram e minimizam; arrastar uma janela maximizada ou encaixada pela barra a solta sob o
  ponteiro. Geometria pura em `osjeff_core::snap`.

- **Painel superior no lugar da barra de menus.** Sem nome de app em negrito nem menus por app:
  **Apps** e Busca à esquerda, data e hora no **centro** (abre o calendário com o centro de
  notificações: histórico de avisos e erros, *Limpar*, chave *Não perturbe*) e, à direita, a pílula
  de status (rede, aparência, energia) que abre as **Configurações rápidas**, uma grade de blocos
  (Rede, Aparência, Movimento, Não perturbe, Relógio 24 h, Configurações), cores de destaque e
  Reiniciar / Desligar. Isso substitui a barra de menus e o popover de Controles. Botão direito em
  *Apps*: menu do sistema.

- **Barra de tarefas no lugar do dock com ampliação.** Barra flutuante arredondada no centro de
  baixo: botão Apps, apps fixados, apps abertos sem fixar e a faixa *Mostrar área de trabalho*
  (também `Ctrl+Alt+D`). Sem ampliação: o ícone sobe um pouco sob o ponteiro e mostra uma dica;
  uma pílula marca o app em foco e um ponto os demais; clicar foca, restaura ou minimiza,
  `Shift`+clique abre outra janela, arrastar um ícone fixado reordena (os vizinhos deslizam com
  mola) e o botão direito lista as janelas do app com *Fixar* e *Fechar*. Geometria e regras
  puras em `osjeff_core::taskbar` (layout, hit test, indicador, clique, reordenação).

- **Linguagem visual própria.** Ícones de tile quadrado arredondado (22 %), cor chapada com uma
  faceta de destaque e bisel de 1 px, sem gradiente nem brilho; ponteiro novo (seta fina com cauda
  arredondada e contorno índigo); **seis papéis de parede originais** (Crepúsculo com facetas
  geométricas, Aurora, Mono, Papel, Turquesa com colinas, Pôr do sol em faixas), com polígonos
  translúcidos no `osjeff_core::wallpaper`; janelas com raio de 8 px, borda de 1 px com realce
  interno e sombras menores. A galeria (`Ctrl+Alt+G`) ganhou a aba *Shell* com botões de janela,
  blocos, indicadores, encaixe e ponteiros.

- **Apps com trilho de categorias.** Trilho à esquerda (Todos, Sistema, Internet, Mídia,
  Utilitários, com a contagem de cada uma), busca no topo, linha **Recentes** (os cinco últimos apps
  abertos) e a grade; `Ctrl`+setas percorrem o trilho. Categorias numa tabela embutida e filtro,
  ranking e recentes puros em `osjeff_core::launcher`.

- **Áreas de trabalho.** De 2 a 4: `Ctrl+Alt+←/→` trocam (as janelas deslizam), `Ctrl+Alt+Shift+←/→`
  levam a janela focada, o painel mostra pontos clicáveis e o menu da janela tem "Mover para a área de
  trabalho N"; ativar uma janela de outra área (barra de tarefas, `Alt+Tab`) traz a área dela.
  Lógica em `osjeff_core::winman` (`switch_workspace`, `move_to_workspace`, `visible_workspaces`).
## 2026-10 — Arquivos, Imagens, Editor e Terminal (W23)

- **Arquivos** no estilo do Finder: barra lateral (Favoritos, Locais, disco com barra de uso),
  barra de ferramentas com voltar/avançar, caminho clicável, vistas em lista e em ícones, busca que
  filtra a pasta, ordenação pelo cabeçalho, painel de pré-visualização (Espaço), seleção por
  retângulo, arrastar e soltar entre pastas, barra lateral, migalhas e Lixeira (com destaque do
  alvo), renomear no lugar, menus de contexto no estilo novo, rolagem com inércia e barra de
  rolagem sobreposta, cópia com folha de progresso e cancelamento, estados vazios. Lista
  virtualizada: 2000 arquivos rolam sem custo extra por arquivo.
- **Imagens**: faixa de miniaturas, ajustar/preencher/100 % com zoom por mola, arrasto com inércia,
  giro animado, painel de informações translúcido, apresentação (Espaço e setas), xadrez sob a
  transparência, folha de salvar, mensagens de erro em português.
- **Editor**: margem com números de linha, linha atual destacada, guias de indentação, cursor que
  desliza e pisca suave, seleção que aparece, barra de buscar/substituir fina (não modal) com botões,
  barra de estado (Ln/Col, codificação, fim de linha, tamanho), título `Editor — nome •`, a pergunta
  de salvar e os diálogos Abrir/Salvar como folhas presas à janela com o visual do Arquivos.
- **Terminal**: faixa com a aba e a pasta, prompt colorido, seleção com o mouse (duplo clique pega a
  palavra, triplo a linha) e cópia com Ctrl+Shift+C, barra de rolagem sobreposta, cursor em bloco ou
  barra. Cores ANSI ficaram de fora.
- **Ctrl + / Ctrl - / Ctrl 0** mudam o tamanho do texto do editor e do terminal; os valores
  (`editor_font`, `terminal_font`) ficam em `/etc/osjeff.conf` (leitor total, com limites).
- Geometria, acerto do mouse, arrastar e soltar, filtro, pré-visualização, miniaturas, inércia,
  apresentação, seleção do terminal e as folhas são lógica pura no `osjeff_core`
  (`fileman::ui`, `viewer::ui`, `editor2::ui`, `termui`, `appart`), testada no host; o kernel só
  desenha. Nenhum `text::legacy` restou nesses apps.
- Testes: 2590 no `osjeff_core` (eram 2497); custo de quadro e capturas em `docs/TESTING.md` e
  `docs/design/ui-macos.md`.
## 2026-10 — Navegador novo (W24)

- **Páginas com texto proporcional.** O layout mede cada palavra na fonte real (Inter, JetBrains
  Mono em `pre`/`code`), com tamanhos, negrito, itálico (inclinação sintetizada), quebra pela
  largura medida, alinhamento, altura de linha, listas, citações, tabelas, caixas em linha,
  formulários no estilo do sistema e os estilos de fonte, cor, fundo, margem, borda e `display`.
- **Moldura nova:** barra única (voltar, avançar, recarregar/parar, campo com indicador de
  segurança e estrela, progresso), balão do certificado (host, emissor, validade), sugestões em
  vidro, **abas** (até 8, Ctrl+T/W/Tab/1..9), tela de **Nova aba**, páginas de erro com
  "Tentar novamente", busca na página, pílula de zoom, rolagem com inércia, menu de contexto.
- `osjeff://favoritos`, `historico` e `sobre` viraram HTML e CSS pelo mesmo motor.
- Rolar, passar o mouse e digitar no navegador repintam só a área do cliente; o layout de uma
  página de 2000 nós caiu de 62,7 para 25,4 ms (cache de glifos, índice de regras, menos alocações).
- Corrigido: a palavra de mais de 4096 caracteres contava entre palavras e cortava palavras
  comuns de textos longos; células de tabela deslocavam as vizinhas; bordas recolhidas perdiam
  o topo das linhas.
- Testes: 2631 no `osjeff_core` (eram 2497).

## 2026-10 — Nova interface (W22)

- **Visual novo, claro e escuro.** Barra de menus com menu do sistema, nome e menus do app em
  foco (ligados a ações reais), rede, Controles, Busca e relógio com data; barra de apps flutuante
  com ampliação por mola, pontos de app aberto, dicas e salto ao abrir; overlay **Apps** no lugar
  do painel iniciar; **Busca** (`Ctrl+Space`) com apps, arquivos do volume e uma calculadora;
  janelas com barra de título unificada, botões à esquerda, cantos de 12 px e sombra em duas
  camadas; folha de confirmação para reiniciar e desligar; popovers de Controles e de calendário;
  banners que deslizam no canto superior direito; cursores vetoriais (seta, mão, I). A identidade é
  própria (marca, nomes, ícones, paleta índigo, movimento); detalhes em `docs/design/ui-macos.md`.
- **Texto vetorial em toda parte.** Inter para a interface e JetBrains Mono para o terminal e o
  editor (ambas OFL, em `THIRD-PARTY.md`), por um leitor TrueType e um rasterizador próprios, no
  `osjeff_core`, com atlas lazy (917 glifos, 61 KiB, 34 ms no boot). A fonte 8x8 ficou só na tela
  de pânico.
- **Primitivas e movimento.** Formas com anti-aliasing, sombras analíticas, gradientes, desfoque e
  reamostragem; molas e curvas por tempo real, interrompíveis, com a chave *reduzir movimento*.
  Janelas abrem, fecham, minimizam (voando para o ícone), restauram e fazem zoom animados; o
  desktop ocioso continua custando zero quadros.
- **Configurações:** `appearance` (automática pela hora, clara, escura) e `reduce_motion`, no
  arquivo de configuração (leitor total) e na tela, aplicados na hora; novas cores de destaque e
  papel de parede dinâmico com esquema escuro.
- **Atalhos novos:** `Ctrl+Space` Busca, `Ctrl+W` fechar, `Ctrl+M` minimizar, `Ctrl+Alt+G`
  galeria de componentes, `Ctrl+Alt+H` HUD de desempenho (escondido por padrão).
- **Toolkit** em `desktop/ui.rs` (botões, campos, segmentado, switch, slider, listas, menus, dicas,
  gráficos) com geometria pura em `osjeff_core::{chrome,widgets}`; os apps antigos seguem
  funcionando dentro da moldura nova e seguem a aparência pelas funções de `theme`.
- Testes: 2414 no `osjeff_core` (eram 2331); custo de quadro antes e depois em
  `docs/TESTING.md`.
## 2026-10 — Correções do uso real (W20)

- **Cursor sem rastro.** O sprite do cursor vive só no framebuffer; os caminhos de desenho que
  subiam retângulos sem tratar `cursor_moved` (hover, clique, tecla) deixavam o sprite antigo na
  tela, um rastro de setas sob as janelas. Agora todo quadro que renderiza algo apaga o sprite
  no início e o pinta por último (`osjeff_core::cursor::CursorTrack`, modelo testado no host;
  invariante em `docs/ARCHITECTURE.md` 6.2). Prova no QEMU: 181 pixels fantasma por rodada antes,
  0 depois, em vários ritmos de mouse (`tools/perf/scen/w20-cursor.sh`).
- **Sites reais.** Uma página gzip maior que o limite de resposta ficava cortada no meio do fluxo e
  virava "Falha ao descompactar a pagina". Agora o prefixo decodificado é mostrado com uma faixa
  ("Pagina cortada no limite de tamanho"); CRC errado, conexão interrompida e dados corrompidos
  também mostram o que chegou. `Content-Encoding` em lista, deflate cru ou zlib e
  `Transfer-Encoding: gzip, chunked` entendidos; limites 1 MiB na rede e 4 MiB descompactado
  (orçamento de heap em `browser::MAX_RESPONSE_BYTES`). Novo alvo de fuzz `http_body`.
## 2026-10 — Entropia de verdade (W21)

- **Problema:** no QEMU com WHPX (Windows) a CPU do guest não tem `RDRAND` e o kernel caía num
  misturador de 64 bits (TSC e ticks) para o *client random* e a chave efêmera do TLS, o ISN do TCP
  (que era **zero**), o `xid` do DHCP, o id e a porta do DNS; o toast `RNG: weak fallback`
  aparecia a cada boot.
- **`osjeff_core::entropy`** (puro, `no_std`, `forbid(unsafe)`, sem dependência nova): ChaCha20
  (RFC 8439), DRBG de apagamento rápido de chave com contadores de reseed, pool SHA-256 com crédito em
  milibits e saúde por fonte, estimador de jitter (testes de "preso", variação e repetição) e a nota
  `Quality` (Weak < Mixed < Strong; timing sozinho nunca é Strong). 40 testes novos (vetores da RFC,
  respostas conhecidas calculadas à parte, determinismo, reseed, 1 MiB de saída com bit balance,
  qui-quadrado e transições, varredura de invariantes), mais `rng` (RDSEED, valores sabidamente
  ruins) e os ids PCI do virtio-rng. Novo alvo de fuzz `entropy_api` (14 no total).
- **Kernel:** `rng.rs` (RDSEED/RDRAND com tentativas e checagem de 0/tudo-1/bloco constante; anel de
  timestamps **sem alocação e sem trava** preenchido pelas ISRs do timer, teclado e mouse e pela
  chegada de quadros; dobra em contexto de thread; reseed periódico) e `virtio_rng.rs` (driver virtio
  1.x, `1af4:1005`/`1044`). Um só ponto, `rng::fill`, para TLS, ISN do TCP (semente do `smoltcp`),
  DHCP, DNS, SNTP, portas locais e `random_get`/`random` dos apps WASM (antes um xorshift por app).
- **Política:** Strong/Mixed seguem como antes e a mudança sai **uma vez** como linha INFO
  (`RNG: pool seeded from timing jitter (N bits credited)`), sem toast. Só Weak (< 128 bits) faz o HTTPS
  esperar até 5 s (coletando jitter de CPU) e depois **recusar**, com um aviso: acabou o caminho
  "usa o gerador fraco mesmo assim" (item do ROADMAP).
- **Scripts:** `tools/run.sh`, `tools/qemu-headless.sh`, `tools/perf/run.sh`, `run.ps1` e
  `tools/qemu-up.ps1` passam `-device virtio-rng-pci` por padrão quando o QEMU o tem
  (`QEMU_RNG=none` / `-NoRng` desligam, para exercitar o caminho de jitter).
- **Provado em QEMU** (`design/entropy.md` §5): com virtio-rng `RNG: strong`; sem ele e com
  `-cpu qemu64` o pool passa de 128 bits em ~1-2 s de ticks do timer e o HTTPS chega ao fim da
  verificação de certificado; os *client randoms* capturados no `net.pcap` são todos diferentes entre
  boots e entre conexões.
- Documentação: `design/entropy.md` (novo), SECURITY-MODEL §3.7, ARCHITECTURE §9.1, ROADMAP, TESTING.
- Testes: 2331 -> 2373 no `osjeff_core`; 14 alvos de fuzz.

## 2026-10 — Expansão do sistema: integração (W15–W19)

- Os quatro blocos (arquivos/VFS, editor e terminal, navegador completo, apps e sistema em disco) foram
  integrados na mesma árvore; os pontos de contato foram reconciliados em commits próprios:
  um único caminho da roda do mouse (`Desktop::handle_wheel`, `dz > 0` = para baixo; o decodificador
  PS/2 do W17 ficou), um único mailbox do `fetcher` com três clientes (navegador, apps WASM e shell,
  cada um com seu estado e o resultado só é retirado por quem postou), `DragMode::PageSelect` para a
  seleção do navegador.
- **Favoritos persistentes** em `/home/.bookmarks` (`SavedBookmarks`, texto `url<TAB>título`, leitor
  total e limitado a 64 entradas). Prova em QEMU em dois boots (`w19-bookmark*.sh`).
- **ABI dos apps:** `PageUp`/`PageDown` chegam aos apps WASM como `0x106`/`0x107` (`KEY_PAGE_UP/DOWN` no SDK).
- Testes: 2331 no `osjeff_core`; cobertura de linhas 96,6%; 13 alvos de fuzz.

## 2026-10 — Editor e Terminal de verdade (W15b)

- **Terminal:** o motor `osjeff_core::shell` (52 comandos, pipes, variáveis, scripts) com histórico
  (↑/↓, Ctrl+R), Tab (comandos, variáveis, caminhos), Ctrl+C / Ctrl+L / `clear`, rolagem (PageUp/
  PageDown, roda, Ctrl+Home/End; até 5000 linhas), colar sem executar, prompt colorido e grade que
  acompanha a janela. Sistema de arquivos pelo VFS (`VfsFs`, diretório corrente por terminal; `rm`
  vai para a lixeira); `date`, `uptime`, `free`, `df`, `ps`, `kill`, `ping`, `ifconfig` com dados
  reais (`KSys`); **novos** `nslookup`, `curl`, `wget`, `ifconfig` (`SysInfo::resolve`, `http_get`,
  `net_info`) e `edit`, `files`, `tasks`, `calc`, `reboot`, `shutdown`.
- **Comandos longos sem congelar:** duas threads `shelld` executam as linhas; Ctrl+C cancela
  (`SysInfo::interrupted`, status 130). Nova caixa de correio bloqueante no `fetcher`
  (`fetch::run_job`: DNS e GET) com estado `ABANDONED` para quem desiste no meio.
- **Editor:** `osjeff_core::editor2` no lugar da grade 44x18: números de linha, UTF-8, desfazer/
  refazer, Ctrl+F/H/G, mouse (clique, duplo, triplo, arrastar), roda, arquivos de até 16 MiB (testado
  com 1 MB e 165 mil linhas), Ctrl+O / Ctrl+S / Ctrl+Shift+S pelo VFS (seletor de arquivos com
  confirmação de substituição). **Fechar com alterações pergunta** Salvar / Descartar / Cancelar
  (botão, Ctrl+Q, Task Manager, `kill`, Reiniciar/Desligar); o Arquivos abre texto no mesmo editor.
- **Removidos:** `osjeff_core::editor` e `terminal` (grade fixa). Esc não fecha mais o editor.
- **Achados dirigindo a interface:** `tr` não entendia `\n` (corrigido), o campo "Salvar como"
  anexava ao nome sugerido (digitar agora o substitui), Alt+A/R/C não chegavam ao editor.
- **Testes:** 2071 -> 2076 (+61 novos, -56 dos módulos removidos): `Screen`, `Term`, `Picker`,
  `CloseAsk`, comandos de rede, Ctrl+C. Fuzz: `shell_parse` agora executa uma sessão de terminal
  inteira e os comandos de rede; novo alvo `editor_dialog`. Cenários `tools/perf/scen/w15b-*.sh`
  (`typestr` em `lib.sh`).
## 2026-10 — Apps e gerenciamento do sistema sobre o disco e a rede reais (W18)

- **Apps no disco** (`osjeff_core::appfs::VolumeFs`): `/apps/<id>.wasm`, `/data/<id>` e `/home`
  vivem no volume OJFS v3 (o mesmo do Arquivos; sem disco v3, no volume em RAM do desktop) e
  **persistem entre boots**. O adaptador só alcança essas três árvores, protege as pastas-raiz,
  limita arquivos a 64 MiB e mantém a cota exata depois de um reboot; uma chamada = uma seção
  crítica do `YieldMutex` do volume, sem mascarar interrupções e sem atravessar o guest.
  Provado em QEMU: o Notas salva em `/data/notes`, o sistema reinicia com a mesma imagem e a nota
  abre. `seed_once` (`/apps/.seeded`): um app embutido removido não volta no boot seguinte.
  `fuzz/app_sandbox` também roda sobre `VolumeFs`; 18 testes novos (paridade com `MemFs`,
  remount, cota, disco cheio, só-as-três-árvores).
- **Arquivos, lugar Apps:** barra lateral e tecla `A`; lista pacotes instalados e embutidos com
  ícone, tamanho e estado; `Enter` executa (instalando antes), `I` instala, `Del` remove, menu
  de contexto, erros na linha de estado. **Propriedades** de um app e de qualquer `.wasm` mostram
  o manifesto (permissões e cotas). `Desktop::{app_rows, install_bundled, remove_app}` voltaram a
  ter uso (sem `#[allow(dead_code)]`); arquivos escritos por apps aparecem no Arquivos em 100 ms.
- **Rede dos apps:** `net_http_get` deixou de devolver `ERR_NOSYS`; usa o `fetcher`/`netd`, com a
  política (permissão, filtro de destinos, **`net_hosts`** novo no manifesto) aplicada antes e em
  cada redirecionamento, checagem do endereço **resolvido**, TLS com verificação completa e sem
  "continuar mesmo assim", só corpo 2xx decodificado. Provado em QEMU contra um servidor falso
  (`tools/nettest-server.py`, app de teste `wasm-apps/nettest`). Limite documentado: um app
  esperando a rede atrasa os outros apps.
- **Boot e logs:** a ordem `storage::init` < `Desktop::new` < `load_settings` está documentada e
  provada (imagem de fundo, destaque, relógio 12 h, fuso e ABNT2 voltam depois de reiniciar com o
  mesmo disco); o caminho de papel de parede sem barra (`papel.png`) deixou de falhar; "Salvar"
  escreve `/var/log/syslog.txt`; a nova thread `logd` grava `/var/log/boot.log` (limitado a
  96 KiB, fora do compositor e de IRQ). Falhas de E/S de arquivos do sistema e de ATA agora
  deixam um WARN. O monitor e as configurações leem os contadores de rede de `netd::stats()`
  (o módulo `netstats.rs` saiu; o gráfico agora anda também com virtio-net).
- Testes: 2071 -> 2112 no core (VolumeFs, `seed_once`, lugar Apps, `net_hosts`, `authorize`,
  `app_response`, `dump_bounded`, `absolute_path`); cenários `tools/perf/scen/w18-*.sh`.

## 2026-10 — Desktop sobre o OJFS v3: gerenciador de arquivos e visualizador de imagens

- **VFS do desktop** (`desktop/vfs.rs` + `osjeff_core::vfs`): uma API de caminhos absolutos
  para gerenciador, visualizador, editor e terminal, sobre o OJFS v3 montado (`storage`).
  Disco pequeno/sem disco/desconhecido: volume de 4 MiB na RAM com aviso (v2 de um disco de
  64 KiB é importado). O v2 saiu do desktop (`disk()`, `PERSIST`, `ata::read_image/write_image`);
  a semente de boas-vindas existe num lugar só (`vfs::seed_welcome`).
- **Gerenciador de arquivos v2:** caminhos, migalhas, histórico, barra lateral com uso do disco,
  colunas ordenáveis, seleção múltipla, novo arquivo/pasta, renomear (F2), copiar/recortar/colar
  entre janelas, lixeira (excluir, restaurar, esvaziar), exclusão permanente com confirmação,
  propriedades, menu de contexto, 20 000 linhas, nomes UTF-8 de 255 bytes, cópias grandes em
  passos por quadro com barra de progresso e cancelamento. Corrige o texto claro sobre fundo claro.
- **Visualizador de imagens** (`Kind::Viewer`, no Painel Iniciar; o dock não mudou): PNG/BMP/PPM,
  ajustar/zoom/roda/arrastar/girar/espelhar, próxima/anterior da pasta, informações, fundo xadrez
  para transparência, salvar como PNG/BMP/PPM. Arquivo corrompido vira mensagem na janela.
- **Mouse com roda** (protocolo IntelliMouse do PS/2, negociado no boot) e teclas F2/F5/PageUp/PageDown.
- **Pontos de extensão:** `desktop::apps_hook::{open_wasm, set_wallpaper}` (as frentes de apps WASM e
  de Configurações ligam depois).
- Ferramenta de host `fs3_inject` (injeta arquivos numa imagem v3). Testes: 1546 -> 1680 (+134).
## 2026-10 — Navegador completo e roda do mouse

- **Roda do mouse** no sistema todo: negociação IntelliMouse no PS/2 (pacote de 4 bytes, com
  queda para o de 3), `dz` no `Event::Mouse`, e a rolagem vai para a janela sob o ponteiro
  (Navegador, Task Manager, Arquivos, Editor).
- **Navegador:** imagens PNG/BMP/PPM (`<img>`, `data:` base64, `<a><img>`, no máximo 8 por página,
  512 KiB cada, 2 Mpx, decodificadas na thread de rede, cache LRU de 6 MiB), formulários GET
  (campos editáveis, Tab, Enter, acentos por teclas mortas, query UTF-8), botões de
  voltar/avançar, cursor de mão, sugestões (favoritos e histórico), favoritos (Ctrl+D,
  `osjeff://favoritos`, `BookmarkStore`), páginas `osjeff://`, PageUp/PageDown/Home/End/Espaço,
  busca na página (Ctrl+F), zoom 50-300%, seleção e cópia de texto, título da janela.
- `web::Doc` separa análise e diagramação; `base64` puro; `keymap::Key` ganhou PageUp/PageDown;
  a URL do navegador passou de 220 para 480 bytes.
- Testes: 1695 para 1910 (215 novos); novo alvo de fuzz `html_img_form` (um bug achado e corrigido).

## 2026-10 — Rede gerenciável

- **NIC:** trait `Nic` e `Port` (dono exclusivo, com contadores); drivers `virtio-net`
  (virtio 1.0, QEMU, BIOS e UEFI) e NE2000; escolha no boot, ou sem rede (a navegação falha
  na hora, em vez de "Carregando").
- **`netd`:** um dono único da NIC (a thread `fetcher`), garantido pelo tipo; o compositor
  não toca mais o hardware.
- **DHCP completo:** máquina de lease pura (T1 RENEW unicast, T2 REBIND, expiração, NAK, ACK
  com configuração nova, RELEASE), DNS inteiro da opção 6, retransmissão com recuo.
- **DNS:** resolvedor próprio com cache TTL e failover entre os servidores do lease.
- **Ping:** `netd::ping_start/ping_poll` e `netd::ping`; **estatísticas** por interface
  (`netd::stats`, linha `[trace] net:` com `perf-trace`).
- Testes: 423 → 525; `fuzz/net_parse` cobre a máquina de lease, o DNS e o ICMP.
- `tools/qemu-headless.sh` ganhou `QEMU_NIC` (`ne2k`, `virtio`, `none`); `tools/pcapsum.py`.

## 2026-10 — Plataforma de apps WebAssembly

O WebAssembly deixou de ser "um app embutido numa thread" e virou a plataforma de apps do
SO: pacote com manifesto e permissões, vários apps ao mesmo tempo, instalação e lançador
([`docs/design/apps.md`](docs/design/apps.md), [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) §10).

- **Pacote e manifesto** (`osjeff_core::{wasmsec, appmanifest}`): um `.wasm` com as seções
  `osjeff.manifest` (`chave=valor`: id, nome, versão, permissões `fs`/`net`/`clipboard`,
  quotas, janela) e `osjeff.icon` (PNG até 64x64). Leitor de seções que nunca entra em
  pânico, quotas com teto imposto pelo sistema, alvo de fuzz `app_manifest`.
- **ABI v2** (módulo `osj`): janela, desenho (`blit_rgba`, `draw_image_png`), entrada por
  exports (`on_key/on_text/on_pointer/on_resize/on_tick/on_close`), tempo, `random`, `log`,
  clipboard, **arquivos** com raiz por app (`/data/<id>/`, `/home`), cota de disco e teto de
  descritores (`osjeff_core::appfs`, tabela de caminhos hostis, alvo de fuzz `app_sandbox`) e
  **rede** com filtro de destinos (`osjeff_core::appnet`; o transporte ainda não está ligado).
  O `host.*` v1 segue igual (snake, plasma, DOOM).
- **Execução multi-app**: `AppManager` com uma `Store` por app, thread `appd` em round-robin
  por fatia, contabilidade de CPU/memória/combustível por app, estados e término real
  (trap, falta de combustível, ponteiro inválido encerram só aquele app; a janela mostra "O
  app encerrou: <motivo>"). Cada janela WASM é uma instância do window manager dinâmico,
  redimensionável conforme o manifesto. Task Manager com seção APPS (`R` reinicia).
- **Instalação e lançador**: `/apps/<id>.wasm`, instalador que valida antes de gravar
  (`osjeff_core::appinstall`), Painel Iniciar com os apps instalados (ícone, nome, rolagem),
  vista **Apps** no Gerenciador de arquivos, apps de exemplo semeados no primeiro boot. O
  desktop do boot continua idêntico (dock inalterado, `verify-boot` com 0 pixels).
- **SDK e apps** (`wasm-apps/sdk`): wrappers seguros da ABI v2 e macros `manifest!`/`icon!`/
  `export_app!`; apps `hello`, `clock`, `notes` (arquivos em `/data/notes`), `paint` (BMP);
  `snake` e `plasma` empacotados. Passo a passo em [`docs/BUILDING.md`](docs/BUILDING.md).
- Limitações conhecidas: `/apps` e `/data` vivem em RAM até o `kernel::storage` existir
  (um só ponto de troca, `appfs_backend.rs`); `net_http_get` valida mas não transporta;
  sockets TCP e a roda do mouse (`on_scroll`) não existem; o menu do dock não lista os apps.

## 2026-10 — Gerenciamento do sistema

Log do kernel, monitor de recursos, configurações e notificações
([`docs/design/sysmgmt.md`](docs/design/sysmgmt.md)).

- `klog`: anel de 64 KiB sem alocação (seguro em ISR, com teste de zero alocações e de
  100 000 mensagens), `klog!(Warn, ...)` com a mesma saída serial de antes, espelho de todo
  `serial_println!`, app **Log do sistema** (filtro, busca, salvar).
- **Monitor** (Processos, Desempenho com gráficos de 60 s, Sistema com CPUID/RAM/boot) e
  **Configurações** (papel de parede incluindo imagem PNG/BMP/PPM do disco, destaque,
  relógio 12/24 h, fuso, ajuste do RTC, teclado ABNT2, rede, armazenamento, energia),
  persistidas em `osjeff.conf`; toasts para WARN/ERROR e `notify!`.
- Traits para outras frentes (`DiskUsage`, `NetStats`, `NetControl`, `LogSink`,
  `SettingsStore`); o dock e o desktop ocioso continuam idênticos.

## 2026-10 — Window manager dinâmico

O desktop deixou de ter 7 janelas fixas (uma por app) e passou a gerenciar qualquer
número de janelas, cada uma com a instância de app e o processo próprios
([`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) §7).

- `osjeff_core::winman`: tabela dinâmica (`WindowManager`, limite 32), `WindowId` forte,
  z-order, foco, abrir/fechar/minimizar/maximizar/restaurar, cascata, mover e
  redimensionar por borda/canto com tamanho mínimo, seletor Alt+Tab em ordem de uso
  recente, duplo clique e assinatura da cena que cobre a geometria. 54 testes novos (423 para 477).
- Vários Terminais, Editores, Gerenciadores de arquivos e Calculadoras ao mesmo tempo,
  cada um com seu processo (`shell`, `shell 2`...). Fechar encerra instância e processo;
  `DEL` no Task Manager fecha de fato a janela. `ProcessTable` passou de 8 para 48 entradas.
- Botões minimizar/maximizar (aparecem com o ponteiro sobre a janela, então o desktop
  parado fica idêntico), duplo clique na barra maximiza, redimensionar por qualquer
  borda, ponto no dock para janelas minimizadas, dock foca em vez de abrir outra janela,
  `Ctrl+N` e "Nova janela" (botão direito no dock) abrem outra instância, Alt+Tab.
- Terminal e Editor usam a maior escala inteira que cabe na janela; Task Manager rola;
  o navegador diagrama a página de novo ao redimensionar.
- Ferramentas: cenários `tools/perf/scen/w8-*.sh` e `tools/perf/w8-heap.sh` (soak de
  abrir/fechar com a ocupação exata do heap em builds `perf-trace`).

## 2026-10 — Auditoria e endurecimento

Auditoria completa de desempenho, segurança e boas práticas
([`docs/audit/`](docs/audit/RELATORIO.md)), seguida de duas rodadas de correções.
Cada mudança é um commit separado, com teste ou prova em QEMU.

### Armazenamento
- OJFS v3 no kernel: driver ATA de bloco (`AtaDisk`, `BlockDevice` com LBA28 fatiado,
  `FLUSH CACHE`, cede a CPU entre setores) e serviço `storage` (detecta, migra do v2,
  formata ou monta o v3 no boot). O desktop segue no v2. Os runners passam a criar o
  disco do filesystem com 64 MiB (esparso); um disco de 64 KiB continua no v2.

### Segurança
- Parsers de rede, disco e HTML/CSS: 9 bugs achados por fuzzing/leitura e corrigidos
  (`dechunk` com tamanho gigante, HTML aninhado estourando a pilha, `parse_color`
  não-ASCII, overflow de porta de URL e de comprimentos CSS, ARP com buffer pequeno,
  OJFS com `parent` em ciclo, `size` fora do limite, imagem curta). Todos com teste
  de regressão e entrada mínima em `fuzz/regressions/`.
- Limites explícitos: corpo de 256 KiB (HTTP e HTTPS), profundidade 40, 8 000 nós,
  1 000 regras e 2 000 seletores de CSS por página.
- Redirects: resolvidos em `osjeff_core::redirect` (preserva o esquema, bloqueia
  https→http, rejeita caracteres de controle, no máximo 5 saltos).
- HTTPS: rótulo "Conexao nao verificada" na barra de endereço; RNG do handshake por
  `RDRAND` com fallback explícito e registrado.
- WebAssembly: *fuel* por chamada, limite de memória de 24 MiB, término real do app
  (laço infinito, `proc_exit`, falha de carga), tetos nas host functions WASI.
- virtio: validação de `qsize`, BAR e limites de capability antes de tocar MMIO.
- NE2000: teto de `send`, `curr - 1` sem underflow, orçamento de recepção.
- Disco: uma falha de leitura no boot não reescreve mais o filesystem.

### Robustez
- Uma thread secundária (`fetcher`, `wasmapp`) que entra em panic ou exceção **morre
  sozinha**: a serial registra o motivo, o Gerenciador de tarefas mostra `DEAD`, o
  navegador e a janela WASM mostram o erro, e o compositor continua. Compositor, `#DF`
  e falhas com interrupções desligadas seguem fatais.
- Páginas de guarda (desmapeadas) sob as pilhas das threads, com `#PF` em pilha IST
  própria: um estouro, inclusive de frame grande (150 KiB), deixa de corromper o heap
  vizinho.
- GDT/TSS próprias com pilha IST para #DF e pilha de boot de 512 KiB: estouro de
  pilha deixa de ser triple fault mudo.
- Panic, todas as exceções da CPU e OOM imprimem na serial e pintam uma tela de
  erro (`crash.rs`); IRQ 7/15 espúrias são ignoradas (isso também corrigiu o boot
  com `-device virtio-gpu-pci`).
- Recusa limpa de framebuffer maior que os buffers estáticos.
- Gerenciador de arquivos abre e salva o arquivo da pasta certa (antes, sobrescrevia
  o homônimo da raiz).
- Allocator devolve o padding de alinhamento à free-list; `anim_signature` não colide
  com 9 ou mais janelas.

### Desempenho (QEMU sem KVM, razões)
- Scheduler com estado "bloqueada" e vetor de yield: compositor em idle de 83 para
  250 iterações/s; latência tecla→captura de ~11 ms para ~0,4 ms; o app WASM deixa de
  ocupar 1/3 da CPU parado.
- Tick do relógio 15,6 → 0,2 ms; quadro de tecla 26 → 13 ms; preenchimento 24 bpp
  14 → 3 ciclos/px; sombras com tabela de blend (24 → 10 ciclos/px).

### Adicionado
- Rede: o resultado do DHCP (`NetConfig`) configura a pilha TCP/DNS do navegador (antes só
  o responder ARP/ping; o IP do SLIRP era fixo). Literais IPv4 não consultam o DNS.
  Provado em QEMU numa sub-rede `192.168.77.0/24`.
- Editor: arquivo que não cabe na grade 44×18 agora é marcado `TRUNC` e **não pode ser
  salvo** (antes era truncado em silêncio e o save destruía o resto).
- `osjeff_core::{hw, layout, wm, gfx, redirect, rng}` (lógica movida do kernel, com
  testes); `fs::read_in`, `fs::live_dir`.
- `tools/`: `qemu-headless.sh`, `verify-boot.sh`, `run.sh`, harness `perf/` e
  `bench/` (criterion, fora do workspace); `fuzz/` com 3 alvos.
- CI (`.github/workflows/ci.yml`), `deny.toml`, `#![warn(clippy::undocumented_unsafe_blocks)]`.
- Documentação: `BUILDING`, `TESTING`, `CONTRIBUTING`, `SECURITY`, `SECURITY-MODEL`,
  `ROADMAP`, o ADR de isolamento e os relatórios de auditoria.

### Alterado
- Toolchain fixado em `nightly-2026-10-05` (o `nightly` sem data quebrou o build);
  `bootloader` 0.11.17, `x86_64` 0.15.5, `spin` 0.9.9, `anyhow` 1.0.104 e demais
  atualizações compatíveis.
- `os/` roda o QEMU com 256 MB (UEFI entrava em pânico com 128 MB: o BSS do kernel
  tem ~91 MiB).
- Todo bloco `unsafe` do kernel tem `// SAFETY:` (de 100 sem comentário para 0).
- Testes: 189 → 423. `cargo fmt` e `cargo lint-*` passam e são exigidos no CI.

### Conhecido e ainda aberto
HTTPS sem verificação de certificado; sem ring 3; thread morta não é reiniciada nem libera
recursos; lease DHCP sem renovação e um único driver de NIC (NE2000); nenhum teste em
hardware real. Ver [`docs/ROADMAP.md`](docs/ROADMAP.md).
