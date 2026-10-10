# Usuários e segurança: o plano

Este documento organiza, em fases com critério de aceite, como o Kitsune passa de "tudo em ring 0, sem
contas" para um sistema com usuários, permissões e processos isolados. O estado atual e o que **não** é
protegido estão em [`../SECURITY-MODEL.md`](../SECURITY-MODEL.md); este documento diz o que muda e em que ordem.

## Princípio

Cada fase entrega algo útil sozinha e **não promete mais do que prova**. Enquanto os apps nativos e o
kernel dividirem o ring 0, as contas e permissões protegem os usuários **uns dos outros dentro do que o
sistema mediar** (arquivos, apps WebAssembly), mas um bug no kernel ignora tudo isso. A fase 3 (ring 3) é
a que muda isso, e a documentação diz essa diferença com todas as letras em cada fase.

## Fase 1: contas e permissões (sem ring 3)

| Passo | O quê | Estado |
|---|---|---|
| 1.1 | Grupo `security` no `kitsune_core`: contas (`account`), senha PBKDF2-HMAC-SHA-256 com sal (`password`), regras de permissão `rwx` e *sticky* (`perm`), limitação de tentativas e bloqueio por inatividade (`session`). Tudo puro e testado no host. | feito |
| 1.2 | OJFS v3 guarda `gid` no inode (campo novo em um trecho que estava sempre zerado, então discos antigos continuam válidos, com grupo 0). `stat` devolve dono, grupo e modo; `chmod` e `chown` no VFS. | feito |
| 1.3 | `Secured`: um adaptador do VFS que impõe as regras de `perm` para um `Cred`, inclusive a criação com dono e `umask`, e o *sticky* em pastas compartilhadas. | feito |
| 1.4 | No kernel: banco de contas em `/etc/accounts` (criado na primeira inicialização, com um assistente de criação do primeiro usuário), tela de login, tela de bloqueio, `fs=home` apontando para a pasta de quem entrou, Terminal com `whoami`, `id`, `chmod`, `chown`, `passwd`, `su`. | **feito** (sem assistente de primeiro usuário: o `kitsune` sem senha entra sozinho; `passwd` e `su` ficam para depois) |
| 1.5 | Ajustes: página **Usuários** (criar, remover, trocar senha, administrador ou não). Registro de auditoria de login e de mudanças de conta. | **feito**: página Usuários e registro de auditoria (`auth: ...` no Registro e na serial: entradas, recusas, trocas de senha, contas criadas e removidas; nunca senhas) |

*Aceite da fase:* testes no host provando que um usuário não lê, escreve nem apaga o arquivo de outro; que
a senha nunca aparece em disco nem em log; que a décima tentativa errada espera o tempo previsto; fuzz do
parser de contas e do VFS com permissões sem falha.

**O que a fase 1 não faz:** não isola um código em ring 0 de outro. Um driver ou o próprio kernel com um
erro de memória ignora as permissões. Também não criptografa o disco.

## Fase 2: endurecimento que não depende de ring 3

| Passo | O quê |
|---|---|
| 2.1 | Assinatura dos pacotes `.wasm` (Ed25519): o instalador mostra "assinado por X" ou "sem assinatura" e o manifesto pode exigir assinatura de um usuário administrador. |
| 2.2 | HTTPS completo: revogação, HSTS e *pinning* (item 2 do [roadmap](../ROADMAP.md)). |
| 2.3 | Nenhum segredo em memória por mais tempo que o necessário, e o gerador de números aleatórios com a semente salva entre boots. | **parcial**: a senha digitada (login e Ajustes) fica num `Secret` de capacidade fixa, zerado ao limpar ou soltar; o PBKDF2 ainda deixa resíduos na pilha e a semente não é salva |
| 2.4 | Criptografia opcional do volume do usuário, com chave derivada da senha. |

*Aceite:* um `.wasm` com assinatura adulterada é recusado; um certificado revogado é recusado.

## Fase 3: isolamento por processo (ring 3)

É o item 8 do [roadmap](../ROADMAP.md) e o [ADR de isolamento](../audit/adr-isolamento.md): segmentos de usuário
na GDT, tabelas de páginas por processo, `syscall`/`sysret`, e os apps WebAssembly passando a rodar em ring 3
por trás de uma interface de chamadas de sistema que aplica as regras da fase 1 com o `Cred` do processo.

*Aceite:* um app com acesso inválido à memória não derruba o kernel nem os outros apps; um app não abre o
arquivo de outro usuário mesmo tentando o caminho direto.

## Como cada fase muda o `SECURITY-MODEL.md`

Cada passo concluído move uma linha do §3 ("o que não é protegido") para o §2 ("o que o kernel faz"), com a
prova ao lado (teste, fuzz ou medição). Nada é movido sem prova.
