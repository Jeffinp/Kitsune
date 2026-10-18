# Supervisão de threads: reiniciar e limpar o que morreu (ROADMAP item 1)

**Estado: S0 e S1 feitas e provadas no QEMU (W65); S2 a S5 por fazer.** O desenho abaixo foi aprovado com as três respostas: trava de dono morto tomada e envenenada (S4), `crash` só para administrador, aviso na central a cada reinício. Hoje uma thread do kernel que falha morre sozinha e o resto segue
(`kernel/src/sched/death.rs`), mas ela **não volta**: o navegador (fetcher), os apps WASM (appd), o terminal (shelld),
os arquivos de processos (procfsd), o som (audiod) e o log (logd) ficam indisponíveis até o reboot, e tudo o que a
thread usava continua alocado.

## O que acontece hoje (lido no código)

| Fato | Onde |
|---|---|
| Panic, exceção com IF=1 ou canário da pilha pisado marcam `DEAD[slot]`; o slot nunca mais é escalonado | `sched.rs`, `sched/death.rs` |
| A pilha (128 KiB com página de guarda desmapeada) e as alocações **ficam para sempre**: o bloco nunca é devolvido ao alocador porque a página de guarda desmapeada quebraria a lista livre | `sched/spawn.rs` (`alloc_stack`) |
| Só threads de usuário (ring 3) têm reaproveitamento de slot: `Thread.free`, `Thread.block`, `release_dead_user` | `sched/user.rs` |
| `YieldMutex` já não trava a máquina: dono morto devolve `LockError::OwnerDead`, mas a trava **continua presa** e o dado fica inacessível | `sync.rs` |
| O fetcher é uma máquina de estados atômica (`IDLE`, `REQUESTED`, `RUNNING`, `DONE`, `WORKER_DEAD`) mais caixas estáticas; o `Netd` (pilha TCP/TLS) é um `static` que só o worker toca | `fetch/mod.rs`, `fetch/thread.rs` |
| appd é dono de todos os runtimes WASM (`RT`, `SLOTS` estáticos); shelld, procfsd e audiod têm filas e estado estáticos | `wasm/manager*.rs`, `desktop/services/shellhost`, `userfs.rs`, `audio.rs` |
| O compositor (slot 0) nunca é contido: segue com a tela vermelha | `containable()` |

## Modelo de falha

Uma thread morre no meio de qualquer ponto de preempção com IF=1. Fica para trás: (1) a pilha; (2) tudo o que estava
só em variáveis locais (um `Vec`, uma resposta de rede em andamento); (3) travas `YieldMutex` seguradas; (4) estado
compartilhado em `static` no meio de uma transição (o fetcher em `RUNNING`, uma fila com um item "em serviço").
Travas `SpinLock` não entram: elas mantêm IF=0, e `containable()` já recusa conter uma falha com IF=0.

## Desenho

### 1. Serviços reiniciáveis
`kernel/src/sched/supervisor.rs` com uma tabela estática:

```rust
struct Service { name: &'static str, entry: extern "C" fn() -> !, stack: usize, cleanup: fn(), policy: Restart }
```

`spawn_service(&SERVICE)` substitui os sete `spawn` do boot. O compositor não é serviço.

### 2. Quem reinicia
O **compositor**, na volta do laço (`supervise()`, uma leitura atômica quando nada morreu). Nem a thread morta (o
contexto dela foi abandonado) nem a ISR (aloca e libera memória). Para cada slot de serviço morto:

1. `cleanup()` do serviço: o estado compartilhado volta ao inicial e o que estava em voo falha de forma explícita.
2. Liberar a pilha: `vm::remap_page` na página de guarda (já existe) e devolver o bloco ao alocador. O `Thread` guarda
   hoje `block` só para threads de usuário; passa a guardar sempre.
3. Reusar o slot, como `spawn_user` já faz com `free`: pilha nova, área FPU nova, `DEAD`, `WAKE`, `TICKS` zerados.
4. Log `sched: 'fetcher' reiniciado (1 de 3 em 60 s)` e notificação na central.

### 3. Orçamento de reinícios
Lógica pura em `kitsune_core::supervise::RestartBudget` (testada no host): no máximo 3 reinícios em 60 s, com espera de
1 s, 5 s e 30 s entre eles. Estourou: o serviço fica morto e a notificação diz isso. Sem isso um erro determinístico
vira um laço de falhas que gasta a CPU toda.

### 4. Travas de dono morto
`YieldMutex::lock` passa a **tomar** a trava de um dono morto e devolve o guarda com `poisoned() == true` (como o
`Mutex` da `std`), em vez de recusar para sempre. Quem protege dado que pode ficar pela metade (contas, FS) confere o
envenenamento e revalida (o FS tem `fsck`). Pergunta aberta 1 abaixo.

### 5. O que cada serviço limpa
- **fetcher**: pedido em `REQUESTED` ou `RUNNING` termina com falha de rede e `DONE` (quem espera acorda); filas de
  jobs e de scripts são esvaziadas com erro; o `Netd` fecha soquetes, sessões TLS e o pool de conexões (função nova
  `Net::reset_connections`).
- **appd**: todo app em execução vira "travou: serviço reiniciado" e os runtimes (`Store` do `wasmi`) são soltos, o que
  devolve a memória.
- **shelld**: o job em execução termina com erro. **procfsd**: pedidos pendentes recebem erro. **audiod** e **logd**:
  só reiniciam (o estado deles é protegido por `SpinLock` ou é só um ponteiro de fila).

### 6. O que não dá para liberar
As alocações que só existiam em variáveis locais da pilha morta vazam, limitadas a uma requisição. O alocador não sabe
quem alocou o quê, e uma arena por serviço seria uma mudança grande no alocador. Em vez disso: contadores atômicos de
bytes vivos por slot (duas instruções por alocação), para o log dizer **quanto vazou** na morte e para o critério de
aceite medir. Se o número incomodar, a arena por serviço vem depois.

### 7. Teste de aceite e injeção de falha
Comando do Terminal só para administrador, `crash <serviço>`, que faz o serviço entrar em pânico no próximo ponto de
verificação (`sched::inject_fault`). Cenário `w65-restart` no QEMU: matar o fetcher, abrir uma página e ver que carrega;
matar o appd e abrir um app; matar o fetcher quatro vezes seguidas e ver que o orçamento o deixa morto com aviso.
Aceite do ROADMAP: matar o `fetcher` e abrir o navegador de novo funciona sem reiniciar o sistema.

## Etapas e custo (estimativa minha, dias de um desenvolvedor)

| # | Etapa | Dias |
|---|---|---|
| S0 (feita) | `RestartBudget` no core, com testes | 0,5 |
| S1 (feita) | Tabela de serviços, reaproveitar o slot, liberar a pilha, `crash` e log | 2 |
| S2 | Limpeza do fetcher + `Net::reset_connections` + cenário com o fetcher | 2 |
| S3 | Limpeza de appd, shelld, procfsd; cenário com o appd | 2 |
| S4 | `YieldMutex` com envenenamento e contadores de bytes por slot | 1,5 |
| S5 | Documentação (CHANGELOG, TESTING, ROADMAP) | 0,5 |

Total ~8,5 dias. **Natureza: reduz churn e risco** (hoje uma falha do fetcher exige reiniciar o sistema e parece
instabilidade). Custo de oportunidade: o mesmo tempo ficaria para SMP ou o HTTPS completo.

## Riscos
- Liberar a pilha com a ISR ainda olhando o slot: o `Thread` fica no vetor até a reutilização e o `rsp` salvo nunca
  é usado, mas `guard_owner` (chamado do #PF) lê `guard`; a página de guarda só é remapeada depois que o slot sai da
  consulta.
- Um `cleanup` que ele mesmo falha: roda no compositor, então uma falha ali é fatal. Cada limpeza deve ser curta,
  sem alocação em laço e sem esperar outra thread.
- Matar a thread de fora (`kill_user` já faz isso para processos) pega a thread em qualquer lugar, inclusive com um
  `YieldMutex` seguro; é o teste mais duro das travas e entra depois do `crash` determinístico.

## Perguntas em aberto
1. Travas de dono morto: tomar e envenenar (proposto) ou continuar recusando e deixar o dado inacessível até o reboot?
2. O comando `crash` fica no sistema final (administrador) ou só em build de depuração?
3. Avisar o usuário a cada reinício (central de notificações) ou só no log e após o orçamento estourar?
