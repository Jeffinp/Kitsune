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
| SDK e programas | `user-apps/` | `kitsune-user` (chamadas, `print!`, `entry!`), `linker.ld` (uma página por tipo de memória) e os programas `hello`, `spin`, `crash`, `probe` |
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
| 6 | `brk(fim)` | cresce o heap (a partir de `0x1000_0000`, até 8 MiB) com páginas zeradas; devolve o fim do heap, inalterado se recusado |
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

* O Terminal, o Editor, o Navegador, o compositor e os apps v1/WASI **continuam no anel 0**; rodam
  isolados os programas ELF e os apps WebAssembly v2. As funções `osj.*` (e o estado de arquivos, rede e
  desktop) ainda executam no kernel, a serviço do processo, e o `wasmi` do kernel segue compilado para os
  apps que não podem sair dele.
* Sem rede, janelas nem entrada de teclado/mouse para processos ELF genéricos (têm arquivos e argumentos; os
  hospedeiros WebAssembly recebem tudo isso pelo kernel, pelo contrato acima); sem `mmap` nem memória
  compartilhada; sem limite de CPU além da preempção.
* Um fio de kernel por processo de usuário, 8 processos ao mesmo tempo.
