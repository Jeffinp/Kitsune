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

`rax` = número, `rdi`, `rsi`, `rdx` = argumentos, resultado em `rax` (negativo = `-errno`).

| N | Chamada | Efeito |
|---|---|---|
| 0 | `exit(status)` | encerra o processo |
| 1 | `write(fd, ptr, len)` | fd 1 ou 2; até 4096 bytes por chamada |
| 2 | `yield()` | passa a vez |
| 3 | `getpid()` | |
| 4 | `sleep_ms(ms)` | até um minuto, bloqueia a thread sem gastar tempo |
| 5 | `uptime_ms()` | |
| 6 | `brk(fim)` | cresce o heap (a partir de `0x1000_0000`, até 8 MiB) com páginas zeradas; devolve o fim do heap, inalterado se recusado |

Todo ponteiro é traduzido pelas tabelas do próprio processo, página por página
(`AddressSpace::copy_in`), antes de o kernel tocar nele: um ponteiro ruim é um erro de retorno
(`-14`), nunca uma falta no kernel. Uma chamada desconhecida devolve `-38`.

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
* Sem arquivos, rede, janelas nem entrada para processos ELF genéricos (os hospedeiros recebem tudo isso
  pelo kernel, pelo contrato acima); sem `mmap` nem memória
  compartilhada; sem limite de CPU além da preempção.
* As permissões de arquivo ainda não valem para eles (não abrem arquivo algum), então o `Cred` do dono
  do processo ainda não é consultado.
* Um fio de kernel por processo de usuário, 8 processos ao mesmo tempo.
