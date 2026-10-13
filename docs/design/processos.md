# Processos em anel 3

Programas ELF que rodam em modo usuário (ring 3), cada um no seu espaço de endereçamento, atrás de uma
interface pequena de chamadas de sistema. É o primeiro passo da fase 3 de
[usuários e segurança](usuarios-seguranca.md) e o MVP descrito no §5 do
[ADR de isolamento](../audit/adr-isolamento.md). O que ele **entrega** e o que **ainda não** está no fim.

## Como é feito

| Peça | Onde | O quê |
|---|---|---|
| GDT com segmentos de usuário | `kernel/src/gdt.rs` | código e dados com DPL 3; `TSS.RSP0` aponta para a pilha de kernel da thread que está rodando |
| Quadros físicos | `kitsune_core::paging::FrameAlloc`, `kernel/src/uvm.rs` | alocador com ponteiro de varredura por região e conjunto de quadros devolvidos (memória não cresce com o tamanho da RAM); todo quadro sai zerado |
| Espaços de endereçamento | `kernel/src/uvm.rs` | uma raiz de tabelas por processo: a metade do kernel é cópia das entradas do kernel (sem bit `USER`, então o anel 3 não enxerga nada dela), a metade do usuário é a entrada 0 (0..512 GiB), que **começa vazia** (as identidades que o bootloader deixou ali não são herdadas); os 4 MiB baixos ficam sem mapa, então um ponteiro nulo falha |
| Carregador | `kitsune_core::elf` | ELF64 estático, só `PT_LOAD`; recusa W+X, segmentos que dividem uma página, fora de `[4 MiB, 1 GiB)`, mais de 16 segmentos, mais de 8 MiB, ponto de entrada fora de segmento executável, `ET_DYN`, `PT_INTERP`, `PT_DYNAMIC`, `PT_TLS` |
| Processos | `kernel/src/userproc.rs` | tabela de processos, `spawn_elf`, `kill`, `reap`, saída capturada (64 KiB) |
| Escalonador | `kernel/src/sched.rs` | cada thread tem `cr3` e `rsp0`; a troca de contexto carrega os dois (`activate`); `spawn_user` reaproveita a vaga de uma thread de usuário já recolhida (16 vagas) |
| Chamadas de sistema | `int 0x80` (gate DPL 3), `kernel/src/switch.s` | mesma máquina da troca de contexto: salva os registradores, o Rust devolve a pilha a retomar (a mesma, ou a de outra thread quando a chamada bloqueia ou encerra) |
| SDK e programas | `user-apps/` | `kitsune-user` (chamadas, `print!`, `entry!`), `linker.ld` (uma página por tipo de memória) e os programas `hello`, `spin`, `crash`, `probe`, `kterm` e `jsrun` |
| Terminal | `run PROGRAMA|ARQUIVO.elf` | inicia o programa, mostra o que ele escreve, devolve o status; Ctrl+C o encerra; `ps` e `kill` enxergam os processos (pids a partir de 5000) |

## Chamadas de sistema

`rax` = número, `rdi`, `rsi`, `rdx`, `r10`, `r8` = argumentos, resultado em `rax` (negativo = `-errno`).

| N | Chamada | Efeito |
|---|---|---|
| 0 | `exit(status)` | encerra o processo |
| 1 | `write(fd, ptr, len)` | fd 1 ou 2; até 4096 bytes por chamada |
| 2 | `yield()` | passa a vez |
| 3 | `getpid()` | |
| 4 | `sleep_ms(ms)` | até um minuto, bloqueia a thread sem gastar tempo |
| 5 | `uptime_ms()` | |
| 6 | `brk(fim)` | cresce o heap (a partir de `0x1000_0000`, até 8 MiB; 64 MiB para o `jsrun` embutido) com páginas zeradas; devolve o fim do heap, inalterado se recusado |
| 7 | `time()` | segundos desde 1970 no relógio de parede, já no fuso escolhido |
| 8 | `language()` | idioma da interface (0 português, 1 inglês) |
| 9 | `unix_ms()` | milissegundos desde 1970 em **UTC** (corrigidos pelo SNTP quando há rede; o relógio local sem correção se não; 0 sem relógio). É o `Date.now()` do `jsrun`; `time()` menos ele dá o fuso |
| 16–21 | `host_*` | só para o hospedeiro WebAssembly (seção abaixo) |
| 32 | `open(caminho, len, flags)` | `O_WRITE` 1, `O_CREATE` 2, `O_TRUNC` 4, `O_APPEND` 8; devolve o descritor (3 em diante, até 16 abertos) |
| 33 | `close(fd)` | |
| 34 | `read(fd, ptr, len)` | fd 0 é a entrada padrão (o que veio num pipe); até 64 KiB; 0 no fim do arquivo |
| 35 | `write(fd, ptr, len)` | fd 1 e 2 são a saída; de 3 em diante, o arquivo aberto |
| 36 | `seek(fd, desloc, origem)` | `SEEK_SET` 0, `SEEK_CUR` 1, `SEEK_END` 2; devolve a posição |
| 37 | `stat(caminho, len, saida)` | 32 bytes: tipo, modo, tamanho, mtime, dono, grupo |
| 38 | `readdir(caminho, len, indice, saida, cap)` | uma entrada por chamada: tipo (1 byte) e nome; 0 depois da última |
| 39 | `mkdir(caminho, len)` | |
| 40 | `remove(caminho, len)` | vai para a lixeira, como em todo lugar do sistema |
| 41 | `rename(de, len, para, len)` | o destino não pode existir |
| 42 | `getargs(ptr, cap)` | a pasta de trabalho e os argumentos, cada um terminado em NUL; devolve o tamanho |
| 48 | `win_open(w, h, titulo, len)` | abre a janela do programa (uma por processo; conteúdo de até 1280 x 800; título em ASCII imprimível, até 64 bytes) |
| 49 | `win_present(ptr, w, h)` | mostra um quadro: `w * h` pixels RGBA, 4 bytes cada, sem folga entre linhas |
| 50 | `win_event(ptr, timeout_ms)` | escreve 16 bytes (`Event`: tipo e três inteiros) e devolve 1; devolve 0 se nada veio no prazo (0 só olha, `u64::MAX` espera sem limite) |
| 51 | `win_title(ptr, len)` | muda o título |
| 52 | `spawn(req, len)` | inicia um filho (programa embutido ou imagem ELF, argumentos, pasta, entrada padrão; até 1 MiB; `pack_spawn`); devolve o pid. Age com a mesma `Cred` do pai |
| 53 | `proc_poll(pid, ptr, cap)` | move até `cap` bytes da saída do filho e diz se ele ainda roda (`pack_poll`: bytes, estado, status) |
| 54 | `proc_kill(pid)` | para um filho |
| 56 | `page_recv(ptr, cap, timeout_ms)` | só o processo de script de uma página do Navegador (`spawn_page`): a próxima mensagem do navegador; devolve o tamanho, 0 se nada veio no prazo (0 só olha, `u64::MAX` espera sem limite), `E2BIG` se não cabe (a mensagem é descartada); os outros processos recebem `ENOSYS` |
| 57 | `page_send(ptr, len)` | idem, uma mensagem para o navegador (até 4 MiB); `EAGAIN` com 8 MiB esperando |

Todo ponteiro é traduzido pelas tabelas do próprio processo, página por página
(`AddressSpace::copy_in`), antes de o kernel tocar nele: um ponteiro ruim é um erro de retorno
(`-14`), nunca uma falta no kernel. Uma chamada desconhecida devolve `-38`.

## Arquivos, argumentos e entrada padrão

`run PROGRAMA [ARGUMENTOS...]` passa ao programa os argumentos, a pasta em que o terminal está (os caminhos
relativos do programa partem dela) e o que veio num pipe (`echo oi | run cat`). O contrato (números, flags,
códigos de erro e registros, `resolve` de caminhos, `pack_args`/`unpack_args`) vive no crate sem dependências
`user-apps/abi` (`kitsune-userabi`), testado no host, fuzzado (`userabi`) e usado pelo kernel e pelo SDK. O SDK tem
`kitsune_user::fs` (`File`, `stat`, `read_dir`, `mkdir`, `remove`, `rename`, `Env`) e três programas de exemplo:
`cat`, `ls` e `fstest`.

* **Com os direitos de quem o iniciou.** O processo guarda as credenciais do usuário da sessão no momento do
  `run` (`Cred`); toda operação passa por `Secured` com elas (`vfs::as_cred`), então ele lê e escreve exatamente
  o que esse usuário pode, e o resultado é `-13` (`EACCES`) quando o dono, o grupo e o modo não deixam. Trocar de
  usuário depois não muda o que o programa pode. Antes de existir conta alguma, tem direitos totais, como o resto
  do desktop nessa fase. A permissão é pedida na abertura (`open` para escrita num arquivo sem permissão falha ali,
  não no primeiro byte).
* **No fio certo.** A chamada roda numa interrupção (sem poder esperar disco nem uma trava que outro segura); ela
  só **enfileira** o pedido e bloqueia o processo, e a thread `procfsd` faz o trabalho em contexto de thread,
  leva os bytes para a memória do processo página por página e responde; a chamada é reiniciada e devolve o
  resultado (o mesmo esquema do hospedeiro WebAssembly). Um `fstest` inteiro (22 passos de criar, ler, anexar,
  buscar, listar, renomear, remover e recusar) leva ~200 ms.
* **Limites.** 16 arquivos abertos por processo, 64 KiB por leitura ou escrita, caminhos de até 1024 bytes, 4 KiB de
  argumentos; `readdir` percorre a pasta a cada chamada (O(n²) para listar tudo), como o `osj.fs_readdir`.
* **Provado.** Cenário `tools/perf/scen/w46-ring3-files.sh`: `cat` de um arquivo, de um pipe e de um arquivo que
  não existe, `ls /`, e o `fstest` com todos os passos `ok`, inclusive "gravar em `/etc/accounts` é recusado".

## Janelas e entrada

Um programa pede **uma janela** com `win_open` e a desenha em memória própria (RGBA); `win_present` entrega o
quadro e `win_event` devolve o teclado, o ponteiro (posição no conteúdo e botões), o novo tamanho do conteúdo
e o pedido de fechar (`EV_KEY`, `EV_TEXT`, `EV_POINTER`, `EV_RESIZE`, `EV_CLOSE`; tabela em
`user-apps/abi`). SDK: `kitsune_user::win` (`Window`, `next_event`, `put`, `fill_rect`); exemplo `gfxdemo`.

* **Para o desktop é um app como os outros.** A janela ocupa um *slot sem runtime WebAssembly* na tabela do
  gerenciador (`wasm/manager/native.rs`): o compositor, a barra de título, minimizar, redimensionar, o Tarefas e
  fechar funcionam como para um app WebAssembly, e o `appd` nunca o executa. Os quadros chegam pela thread
  `procfsd`, que copia os pixels do processo, converte para o formato da tela fora de qualquer trava e os grava na
  superfície de trás em faixas de 32 KiB (a trava só é tomada por faixa); depois troca as superfícies. A entrada
  vai no sentido contrário: o gerenciador a coloca na fila do processo (movimento do ponteiro se funde) e o
  acorda.
* **Fim de vida.** Fechar a janela avisa o programa (`EV_CLOSE`) e, quando a janela some, encerra o processo;
  o processo que acaba (ou é morto, `Ctrl+C` no `run`) deixa a janela mostrando o motivo, como um app WebAssembly.
* **Limites e notas.** Uma janela por processo, quadros de até 1280 x 800, a fila de eventos guarda 256. A tecla
  Esc não chega ao programa (o desktop a consome). Um quadro de tamanho diferente do conteúdo aparece no canto
  superior esquerdo; o programa refaz o desenho ao receber `EV_RESIZE`.
* **Provado.** Cenário `tools/perf/scen/w47-ring3-window.sh`: o `gfxdemo` abre a janela, anima um quadrado,
  a cruz segue o ponteiro, uma tecla muda a barra, o botão de fechar termina o programa e devolve o prompt, e
  `Ctrl+C` mata o processo e a janela mostra o motivo.

## Processos filhos e o Terminal em anel 3

`spawn`, `proc_poll` e `proc_kill` (52–54) deixam um processo iniciar e acompanhar outros. O pedido de `spawn`
(`kitsune_userabi::pack_spawn`: nome, argumentos, imagem opcional, entrada padrão) é enfileirado e a `procfsd`
cria o processo, com a `Cred` do pai e a pasta de trabalho dos argumentos. O pai só enxerga os próprios filhos
(`ESRCH` para os outros) e o kernel **guarda o filho até o pai coletar o fim** (`reap` espera; se o pai some, o
filho é recolhido normalmente), então status e saída não se perdem. A saída chega por `proc_poll` em pedaços de
até 64 KiB; o filho só conta como terminado quando tudo foi lido.

O **`kterm`** (`user-apps/kterm`, `run kterm`) é o Terminal como programa de anel 3: o mesmo motor de shell e
editor de linha do Terminal do desktop (`kitsune_core::shell`: `Term`, `Shell`, `ShellFs`, `SysInfo`), rodando
num processo próprio com janela própria. `ShellFs` fala com as chamadas de arquivo (cada operação checada contra os
direitos do usuário: `echo oi > /x` na raiz dá "Permissão negada"), `SysInfo` com `time`, `uptime`, `spawn`
e `proc_poll` (o `run` do shell funciona, com pipe, argumentos e `Ctrl+C`). Enquanto um comando roda o
processo continua lendo os eventos da janela: `Ctrl+C` cancela, fechar a janela encerra. O desenho usa a fonte
bitmap 8x8 em 2x (`user-apps/glyphs.rs`, a mesma tabela do kernel), então é mais simples que o Terminal do desktop
(sem seleção, sem cursor piscante, sem rolagem do mouse). A pilha do `kterm` é de 512 KiB (o parser e o executor
recursam). Hoje ele é aberto pelo Terminal do desktop (`run kterm`); o desktop ainda não o põe no dock.

Ainda sem: `ps`, `kill`, `free`, `df`, `whoami` e rede (`ping`, `curl`, `nslookup`), `edit`, `files` e `tasks`
(precisam de chamadas que o processo ainda não tem), `chmod`/`chown`, tamanho de arquivo em `ls` custa uma
chamada `stat` por entrada.

## JavaScript em anel 3 (`jsrun`)

`run jsrun arquivo.js` executa um script com o **QuickJS** (Fabrice Bellard, MIT, versão 2026-06-04, sem
modificações em `user-apps/jsrun/vendor/quickjs`) e imprime o que ele manda para `console.log`. Também
`run jsrun -e 'código'`, `echo 'código' | run jsrun` (entrada padrão) e `run jsrun --selftest` (teste embutido
com um benchmark). O dono do projeto autorizou C para isto, desde que rápido: é um interpretador de bytecode
nativo, compilado com `clang -O2` e doubles de hardware (SSE2), não um motor em WebAssembly.

| Peça | Onde | O quê |
|---|---|---|
| Motor | `vendor/quickjs` | ES2023+ completo: classes, campos privados, generators, async/await, Proxy, BigInt, regex com `\p{...}` e lookbehind, Map/Set/WeakMap, typed arrays, JSON. Sem `Intl` |
| Matemática | `vendor/musl-math` | o libm do musl (MIT), só double; `build.rs` compila só as ~50 funções que o `Math` alcança. `sqrt` é uma instrução (`sqrtsd`) |
| libc mínima | `libc/include`, `libc/libc.c`, `libc/printf.c` | os cabeçalhos que o QuickJS inclui (alguns só com tipos), `str*`, `printf`/`snprintf` (inteiros, `%s`, `%p`, e `%f`/`%e`/`%g` pelo dtoa exato do próprio QuickJS), relógio e calendário (`gettimeofday`, `localtime_r` com `tm_gmtoff`, `mktime`), e *stubs* de `pthread` (um fio por processo; `Atomics.wait` não bloqueia). `memcpy`/`memset`/`memmove`/`memcmp` vêm do `compiler_builtins` do Rust |
| Cola | `src/glue.c` | cria runtime e contexto, define `console.log/info/debug/error/warn`, `print` e `performance.now()`, avalia como código global **com `await` no topo**, roda a fila de jobs (promises) até esvaziar e relata o que não foi pego: `Uncaught <erro>` com a pilha (`at f (arquivo.js:1:27)`), promises rejeitadas sem tratador, status 1 |
| Lado Rust | `src/bin/jsrun.rs`, `heap.rs`, `sys.rs` | lê o script (arquivo pelo `kitsune_user::fs`, até 16 MiB, ou a entrada padrão), dá ao C `malloc`/`calloc`/`realloc`/`free`/`malloc_usable_size` sobre o alocador do SDK (cabeçalho de 16 bytes com o tamanho) e as chamadas `kjs_write`, `kjs_utc_ms`, `kjs_tz_offset_secs`, `kjs_uptime_ms`, `kjs_exit` |
| Build | `user-apps/jsrun/build.rs` | `clang --target=x86_64-unknown-none-elf -ffreestanding -nostdlibinc -O2 -msse2 -fno-pic -fwrapv -fno-math-errno ...` em paralelo; os objetos vão para o linker do Rust. `CLANG` escolhe o compilador. O kernel o embute como os outros (`USER_CRATES`) |

Detalhes que importam:

* **Fronteira Rust/C só com inteiros e ponteiros.** O alvo `x86_64-unknown-none` do Rust é *soft-float*; o C usa
  SSE2. Um `f64` atravessando a fronteira chegaria no registrador errado, então nenhuma função entre os dois tem
  `float`/`double` na assinatura (`performance.now()` converte o inteiro dentro do C).
* **SSE no anel 3.** O kernel liga `CR4.OSFXSR`/`OSXMMEXCPT` e desliga `CR0.EM` no boot (`uvm::enable_sse`, antes do
  primeiro `fxsave` do escalonador); sem isso o primeiro `movups` dava `#UD`. O kernel continua soft-float; o
  estado SSE de cada fio já era salvo e restaurado (`sched::FxArea`).
* **Pilha alinhada.** O `_start` do SDK (`entry!`) passou a ser uma função *naked* que alinha a pilha a 16 bytes e
  *chama* o `main`: o kernel entra com a pilha alinhada (como o Linux), mas código compilado espera 8 bytes de
  diferença na entrada de uma função; com o Rust *soft-float* isso nunca aparecia, com o C usando `movaps` na pilha
  dava `#GP`.
* **Limites.** Pilha de **4 MiB** (`BIG_STACK`; o QuickJS gasta ~1 KiB de pilha C por chamada JS, então cabem ~3700
  chamadas aninhadas antes do `RangeError`), heap de até **64 MiB** (`BIG_HEAP`), memória do JS limitada a 48 MiB
  (`JS_SetMemoryLimit`: estourar vira `InternalError: out of memory`, o processo termina com status 1 e o sistema
  segue). Pilha e heap maiores valem só para a imagem embutida com aquele nome, não para um arquivo `jsrun.elf`
  qualquer. A pilha é mapeada inteira na criação (1024 quadros).
* **Recursão.** O QuickJS para a recursão sem fim com `InternalError("stack overflow")` (como o SpiderMonkey); a cola
  faz `InternalError` herdar de `RangeError`, então `e instanceof RangeError` vale como no V8 e o código escrito
  para ele funciona.
* **Data e hora.** `Date.now()` vem da chamada 9 (UTC); o fuso é `time()` menos ela, arredondado ao quarto de hora
  (sem horário de verão: o fuso do sistema é um deslocamento fixo).
* **Saída.** Cada `console.log` é uma chamada `write` (aparece no Terminal e no `serial.log` como `[jsrun:PID] ...`).
* **Uma corrida corrigida.** Um processo novo podia rodar antes de entrar na tabela de processos (ou achar na tabela
  o dono anterior da mesma vaga de fio, quando o `reap` liberava a vaga antes de tirar a entrada): as chamadas acham o
  processo pela vaga, então as primeiras falhavam em silêncio e o `exit` virava "foi interrompido". Agora o
  `spawn` põe a entrada na tabela na mesma seção crítica que torna o fio executável, e o `reap` libera a vaga e
  tira a entrada juntas. O W51 pegou isso (um `echo ... | run jsrun` saía morto sem saída).

**Provado em QEMU** (`tools/perf/scen/w51-jsrun.sh`, com `QEMU_MEM=256M`; veredito em `$OUT/verdict.txt`): o
`--selftest` passa **56/56** verificações (aritmética e conversões número↔texto, `Math` sobre o musl, maiúsculas com
acento, *astral planes*, `normalize`, template literals, JSON, `Date.now()` plausível e `toISOString`, fuso em minutos
inteiros, regex com grupos nomeados, `\p{L}`, lookbehind e `matchAll`, classes com campos privados e `super`,
generators e `yield*`, Map/Set/WeakMap, BigInt, desestruturação, `?.`/`??`, typed arrays e `DataView`, Proxy/Reflect,
`try/finally`, pilha de erro, promises, `async/await`, `Promise.allSettled`, `for await`, recursão de ~3700 chamadas que
vira `RangeError` sem derrubar o processo); um arquivo escrito no Terminal (`echo '...' > t.js; run jsrun t.js`); um
erro não pego (mensagem, pilha, status 1); script pela entrada padrão com `await` no topo; `-e` com BigInt; arquivo
que não existe; um script que come memória até o limite (exceção, status 1) seguido de outra execução que funciona.

**Velocidade.** Sob QEMU **TCG** (emulação, sem KVM neste ambiente), no `--selftest`: `fib(27)` recursivo
**300–390 ms**, laço de 5 milhões de somas **2,5–2,9 s**, 100 mil objetos criados e ordenados **~2,9 s**. O mesmo
QuickJS com o mesmo script, nativo no hospedeiro (Linux, clang -O2, a CPU deste ambiente), faz `fib(27)` em
**~20 ms**, o laço em **~120 ms** e os objetos em **~85 ms**. O fator de ~15× (cálculo) a ~35× (objetos: muito acesso
à memória, que é o mais caro de emular) é o custo da emulação TCG, não do motor: em hardware real ou com KVM o
`jsrun` deve ficar perto dos números do hospedeiro, que são os normais de um interpretador sem JIT (um motor com
JIT, como o V8, é bem mais rápido nesses laços). O alocador por baixo (`linked_list_allocator` do SDK, primeiro
encaixe) não foi medido à parte; o QuickJS já agrupa os objetos pequenos em arenas de 4 KiB.

**Modo página (`jsrun --page`).** O Navegador inicia um `jsrun` por aba cuja página tem scripts
(`userproc::spawn_page`, com a `Cred` da sessão). Nesse modo o programa avalia primeiro o DOM em JavaScript
(`src/bin/jsrun/dom.js`) e entra no laço de `src/page.c`: espera uma mensagem (chamada 56) ou o próximo timer, roda as
tarefas vencidas com **3 s cada** (o interrupt handler do QuickJS interrompe a que passar disso; o processo e a página
seguem), os jobs de promise, e devolve o documento pela chamada 57 quando mudou. O desenho, as mensagens e as APIs
estão no [`tls-browser.md`](tls-browser.md) §12; prova no cenário `w52-browser-js.sh`.

**Ainda não.** `Intl` (datas e números localizados), módulos (`import`), `fetch`/`XMLHttpRequest`. Sem `test262`.
Fora do modo página não há `setTimeout` nem DOM.

## O que foi provado em QEMU (cenário W43)

* `run hello`: roda em anel 3, imprime no Terminal, dorme, sai com status 0.
* `run probe`: `write` com ponteiro do kernel (`0xFFFF8000...`) ou nulo volta `-14`; fd inválido volta
  `-9`; chamada 999 volta `-38`; a instrução privilegiada `cli` mata **só** o processo com `#GP`.
* `run crash`: escrever no endereço 0 mata só o processo com `#PF` (`USER_MODE | CAUSED_BY_WRITE`); o
  desktop segue vivo e o Registro mostra o motivo.
* `run spin`: laço infinito é preemptado pelo relógio (a interface continua respondendo) e Ctrl+C o
  encerra.
* 20 processos que falham em sequência: os quadros em uso voltam a **0** (nada vaza) e a vaga da thread
  é reaproveitada.

## Apps WebAssembly em anel 3

Os apps v2 (`osj.*`: Relógio, Notas, Pintura e os que o usuário instalar) rodam, por padrão, **cada um
num processo de anel 3 próprio**. O interpretador (`wasmi`), a memória linear do app e o combustível
ficam no processo (`user-apps/wasmhost`, ~0,8 MiB, embutido no kernel); um defeito no interpretador ou
no módulo derruba esse processo e mais nada. O que precisa de autoridade fica no kernel: o estado do
host (superfície onde o app desenha, arquivos, rede, área de transferência) e a implementação de cada
função `osj.*`.

| Peça | Onde | O quê |
|---|---|---|
| Contrato | `kitsune_core::appabi::remote` | tabela `HOST_FNS` (a posição é o id na chamada), números 16–20, registros `Job`, `HostCall`, `Done` (little endian, testados no host) |
| Hospedeiro | `user-apps/wasmhost` | lê o módulo (`SYS_HOST_MODULE_*`), espera trabalho (`SYS_HOST_JOB_WAIT`); cada import do convidado vira um `SYS_HOST_CALL`; termina com `SYS_HOST_JOB_DONE` |
| Lado do kernel | `kernel/src/wasm/remote.rs` | a thread `appd` posta um `Job` (`INIT`, ou `CALL` de um export com combustível) e atende as chamadas até o `Done`; lê e escreve a memória do convidado pelas tabelas de páginas do processo, uma página por vez |
| Mesma implementação | `kernel/src/wasm/abi2.rs` | `abi2::call(&mut dyn Guest, id, args)` serve o app local (um `Caller` do wasmi) e o remoto (as tabelas do processo): validação de ponteiros, limites, permissões e cotas são **um código só** |
| Bloqueio | `kernel/src/userproc.rs` | as chamadas 18 e 19 bloqueiam o processo e são **reiniciadas** ao acordar (`rip -= 2`, o número volta em `rax`): sem estado escondido na pilha de kernel |
| Ciclo de vida | `RemoteRt` | fechar o app, reiniciá-lo, ou o processo morrer (`kill` no Terminal, falha de CPU) termina o outro lado; o `appd` percebe a morte de um processo ocioso a cada tick e a janela mostra o motivo |

Detalhes que importam:

* **Combustível.** Dentro do processo é o `wasmi` que conta, como antes. O trabalho que o kernel faz em
  nome do convidado (preencher retângulos, decodificar PNG, E/S de arquivo) é cobrado do mesmo orçamento
  do job; estourou, o job termina como "sem combustível".
* **Memória.** O heap do hospedeiro é `mem_bytes` do app + 4× o módulo + 6 MiB (no máximo 96 MiB); a
  pilha tem 1 MiB (o tradutor do `wasmi` recursa). Cada hospedeiro custa ~1,2 mil quadros (~5 MiB).
* **Falhas.** Ponteiro inválido, trap, sem combustível e `exit` terminam só aquele app. Um processo que
  para de responder por 20 s é morto.
* **Reserva.** Se não há vaga de processo ou de thread (8 processos, 16 threads), o app roda no kernel
  como antes; o log diz. Apps v1 (cobrinha, plasma, DOOM) e WASI continuam no kernel.
* **Custo e como ficou barato.** Medido com `tools/perf/scen/w45-wasm-cost.sh` (build `perf-trace`, linha
  `[trace] wasm:`; QEMU com `-icount shift=0`, que conta instruções e dá números repetíveis: sem ele o TCG
  varia ~2× de uma execução para outra com a carga do host, e comparar duas imagens vira ruído. A
  variante no kernel se compila com `KITSUNE_APPS_LOCAL=1`). A primeira versão custava ~8,6× o app no kernel
  por job, porque cada chamada era duas trocas de contexto atrás de qualquer outra thread. Hoje o custo
  estável é ~1,2× (291 contra 242 kciclos por job estável; 1,08× contando o trabalho único): (1) **passagem direta** (`sched::hand_off`): o
  processo e o `appd` passam a CPU um ao outro sem esperar a vez na fila; (2) **lote**: `fill_rect` e
  `request_redraw` (75 % das chamadas) não têm resultado nem ponteiro, então o hospedeiro os junta e
  manda em uma troca (`SYS_HOST_BATCH`, até 64; ordem preservada, descarregados antes de qualquer
  chamada que devolve valor e antes do fim do job); (3) `get_size` e `lang` valem o job inteiro e vão
  dentro do `Job`; (4) thread de kernel **não recarrega o CR3** (`sched::activate` empresta a raiz do
  processo, que tem a metade do kernel; `release_cr3` desfaz o empréstimo antes de liberar a raiz); (5)
  o hospedeiro é compilado com `opt-level = 3`.
* **Verificação.** Cenário `tools/perf/scen/w44-wasm-ring3.sh`: Notas, Pintura e Relógio em processos
  (`ps` mostra `app:notes`, `app:paint`, `app:clock`), `kill` de um deles só derruba o app dele, fechar a
  janela devolve os quadros do processo.

## Proteções de hardware

SMEP e SMAP são ligados no boot quando a CPU os tem (verificado no QEMU com `-cpu max`: o log diz
`cpu: SMEP on, SMAP on`): o kernel passa a falhar se algum dia executar, ler ou escrever uma página
marcada de usuário. O kernel nunca faz isso (todo acesso passa por cópias traduzidas), então não há
janelas `stac`/`clac`. Também: NX nas pilhas e no heap do processo, W^X imposto pelo carregador, e um
`mem` de teste mostra que tocar a página logo depois do heap mata só o processo.

## O que ainda não está

* O Terminal do desktop, o Editor, o Navegador, o compositor e os apps v1/WASI **continuam no anel 0**; rodam
  isolados os programas ELF e os apps WebAssembly v2. As funções `osj.*` (e o estado de arquivos, rede e
  desktop) ainda executam no kernel, a serviço do processo, e o `wasmi` do kernel segue compilado para os
  apps que não podem sair dele.
* Sem rede para processos ELF genéricos (têm arquivos, argumentos, janela e entrada; os hospedeiros
  WebAssembly recebem tudo isso pelo kernel, pelo contrato acima); sem `mmap` nem memória
  compartilhada; sem limite de CPU além da preempção.
* Um fio de kernel por processo de usuário, 8 processos ao mesmo tempo.
