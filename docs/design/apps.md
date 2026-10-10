# Plataforma de apps do Kitsune (WebAssembly)

Estado: **implementado** (W13) e **ligado ao disco e à rede reais** (W18: §5, §7 e §8 dizem o
que mudou; a seção "O que vira teste" lista o que cada parte prova). Escopo: transformar o WebAssembly (hoje: um app embutido, preso à
thread `wasmapp`, janela de tamanho fixo, sem arquivos nem rede) em uma plataforma com
**pacote**, **manifesto**, **permissões**, **quotas**, **ABI v2**, **vários apps ao mesmo
tempo**, **instalação** e **lançador**.

A lógica de decisão fica em `kitsune_core` (puro, `no_std`, `forbid(unsafe_code)`, testado no
host e fuzzado). O kernel só liga: instâncias `wasmi`, threads, pixels.

```
kitsune_core::wasmsec      leitor seguro de seções do binário .wasm (nunca panica)
kitsune_core::appmanifest  manifesto `kitsune.manifest` + ícone `kitsune.icon` + política de quotas
kitsune_core::appfs        caminhos, trait AppFs, MemFs (testes), VolumeFs (o volume OJFS v3), Sandbox (raiz por app, descritores, cota)
kitsune_core::appnet       política de rede: URL, filtro de destinos, limites
kitsune_core::appabi       constantes da ABI v2 (erros, flags), validação de ponteiros do guest
kernel/src/wasm/          AppManager (instâncias, appd, host functions `osj.*`, `host.*` v1)
wasm-apps/sdk             crate `no_std` para wasm32: wrappers seguros da ABI v2 + macro de manifesto
```

## 1. Pacote: um arquivo `.wasm` = um app

O pacote é o próprio módulo WebAssembly. Metadados vão em **seções customizadas** (id 0), que
qualquer runtime ignora:

| Seção | Conteúdo | Obrigatória |
|---|---|---|
| `kitsune.manifest` | texto UTF-8, `chave=valor` por linha | sim (para instalar) |
| `kitsune.icon` | um PNG (até 64x64, RGBA/paleta/cinza; decodificado com `kitsune_core::image`) | não |

Sem JSON nem serde. O leitor de seções (`wasmsec`) é a única porta de entrada do binário
não confiável antes do `wasmi`: confere o cabeçalho (`\0asm` + versão 1), caminha pelas seções
(`id`, tamanho em LEB128 de até 5 bytes, limitado ao que sobra no buffer), nunca aloca (devolve
fatias do próprio buffer) e limita: 4096 seções, nome de seção <= 64 bytes, manifesto <= 4 KiB,
ícone <= 64 KiB. Qualquer inconsistência é um `Err` tipado; nunca pânico.

Um módulo **sem** manifesto (DOOM, `cdemo`, builds antigos) continua rodando como app
legado: manifesto sintético (`abi=1`, `fs=none`, `net=none`, `clipboard=none`, janela 692x414
fixa), exatamente o comportamento de hoje.

## 2. Manifesto

Uma chave por linha, `chave=valor`; linhas vazias e `#` comentário são ignoradas; CR final é
descartado; máximo de 64 linhas e 4096 bytes; **chave repetida, chave desconhecida, valor
inválido e permissão inexistente são erros** (a exceção são chaves `x-...`, reservadas a
extensões e ignoradas). Obrigatórias: `id`, `name`, `version`.

| Chave | Valor | Padrão |
|---|---|---|
| `id` | `[a-z0-9._-]{1,32}`, começa por letra ou dígito, sem `..` | — |
| `name` | 1 a 24 caracteres ASCII imprimíveis, sem espaço nas pontas. É o nome de reserva (em inglês nos apps que acompanham o sistema) | — |
| `name.<idioma>` | o nome no idioma (`name.pt=Relógio`, `name.en=Clock`, também `name.pt-br`): 1 a 24 caracteres, com acentos, sem controle nem espaço nas pontas; até 8 linhas; um idioma que o sistema não tem é guardado e não usado. O sistema mostra o do idioma em uso (primeiro a etiqueta inteira, depois a parte principal) e, sem ele, `name` | — |
| `version` | `N.N.N` (cada N <= 65535) | — |
| `abi` | `1` (módulo `host`) ou `2` (módulo `osj`) | `2` |
| `fs` | `none` \| `own` \| `home` | `none` |
| `net` | `none` \| `http` \| `tcp` | `none` |
| `net_hosts` | lista de destinos permitidos, separados por vírgula: `api.exemplo.com` (exato) ou `*.cdn.exemplo.org` (subdomínios; o próprio `cdn.exemplo.org` não entra); no máximo 8, minúsculos, nomes públicos (o que o filtro de destinos recusa não pode ser listado), só com `net=http`/`tcp`. Vazio = qualquer destino público | vazio |
| `clipboard` | `none` \| `rw` | `none` |
| `mem_mib` | memória linear máxima, 1..=24 (teto do sistema) | 8 |
| `fuel_frame` | combustível por chamada ao guest, 10 000..=20 000 000 (teto do sistema) | 4 000 000 |
| `disk_kib` | cota de disco, 0..=4096 (0 se `fs=none`) | 256 |
| `max_fds` | descritores abertos, 1..=32 | 16 |
| `tick_ms` | período de `on_tick` (só com a janela visível); 0 = sem ticks, senão 16..=60000 | 0 |
| `win_w`, `win_h` | tamanho padrão da **área de conteúdo**, 64..=1280 x 64..=800 | 640 x 400 |
| `win_min_w`, `win_min_h` | tamanho mínimo, 64..=win_w/h | min(win_w,200) x min(win_h,120) |
| `resizable` | `0` \| `1` | `1` |

**O teto superior é imposto pelo sistema:** o manifesto *pede*; `Quotas::grant(&manifest)`
devolve `min(pedido, teto)` e o instalador **recusa** (não rebaixa em silêncio) um pedido
acima do teto. Dois níveis: o *instalador* valida e recusa; o *AppManager* aplica o teto de
novo ao iniciar (defesa em profundidade: um arquivo copiado à mão para `/apps` não escapa).

## 3. Permissões

| Permissão | Efeito |
|---|---|
| `fs=none` | toda função `fs_*` devolve `ERR_PERM` |
| `fs=own` | `/` do guest = `/data/<id>/` (criada na primeira escrita) |
| `fs=home` | `/` do guest = a pasta do usuário que entrou (`/home/<nome>`; `/home` antes de alguém entrar) |
| `net=http` | `net_http_get` liberado, com o filtro de destinos abaixo e, se houver, a lista `net_hosts` (fora dela: `ERR_PERM`) |
| `net=tcp` | implica `http`; reservado para sockets TCP (**fora do escopo desta entrega**: a permissão é aceita e validada, mas a ABI v2.0 não exporta sockets) |
| `clipboard=rw` | `clip_get` / `clip_set` |

Negar é sempre um **código de erro** devolvido ao guest (`ERR_PERM`), nunca um trap: o app
decide o que fazer. Não existe permissão que dê acesso ao disco inteiro, ao FS do sistema, a
`/apps`, a `/data/<outro-id>` ou a memória do kernel.

## 4. ABI v2 (módulo de import `osj`)

> `osj` é uma abreviação histórica (do tempo em que o sistema se chamava OSjeff) e **fica como o nome do módulo**:
> trocá-lo quebraria todo `.wasm` já compilado. As seções de metadados passaram a se chamar `kitsune.manifest` e
> `kitsune.icon`; o instalador ainda aceita `osjeff.manifest` e `osjeff.icon`.

O módulo `host` (v1: `log`, `fill_rect`, `draw_text`, `blit`, `time_ms`, mais o subconjunto
WASI) segue **idêntico** para snake, plasma, DOOM e `cdemo`. Um app `abi=2` importa `osj.*`;
nada impede importar os dois.

Convenção: tudo `i32` salvo indicação; resultado `>= 0` é sucesso (contagem, descritor);
`< 0` é erro:

| Código | Nome | Significado |
|---|---|---|
| -1 | `ERR_PERM` | permissão negada, ou caminho tentou sair do sandbox |
| -2 | `ERR_NOENT` | não existe |
| -3 | `ERR_BADF` | descritor inválido / modo errado |
| -4 | `ERR_INVAL` | argumento inválido (nome, tamanho acima do teto, flags) |
| -5 | `ERR_EXIST` | já existe |
| -6 | `ERR_NOSPC` | cota de disco esgotada |
| -7 | `ERR_MFILE` | descritores demais |
| -8 | `ERR_NOTDIR` / -9 `ERR_ISDIR` / -10 `ERR_NOTEMPTY` | tipo errado de entrada |
| -11 | `ERR_NET` | falha de rede ou destino recusado pelo filtro |
| -12 | `ERR_NOSYS` | recurso indisponível neste build |

**Ponteiros e tamanhos do guest são validados por toda função** (`appabi::check_range`):
`ptr + len` com aritmética sem estouro e dentro da memória linear *atual*. Fora da memória
é uma **falta do guest**: a função devolve um trap e **só aquele app** encerra com o motivo
"ponteiro inválido" (um SDK correto nunca produz isso; é bug ou ataque). Tamanho acima do teto
da função (ex.: `log` > 512 B, `fs_read` > 64 KiB por chamada, caminho > 256 B) é `ERR_INVAL`
(sem trap; leituras/escritas grandes são divididas pelo SDK). Todo laço do host que o
combustível não vê é cobrado (ver §6) e tem teto.

| Grupo | Função | Notas |
|---|---|---|
| janela | `set_title(ptr,len)` | <= 48 B, UTF-8. Os apps que acompanham o sistema não chamam: a janela mostra o nome do manifesto no idioma em uso |
| | `get_size() -> i64` | `(w << 32) \| h` da área de conteúdo |
| | `request_redraw()` | agenda um `render` |
| desenho | `fill_rect(x,y,w,h,rgb)` | coordenadas do conteúdo, recorte à superfície |
| | `draw_text(x,y,ptr,len,rgb,scale)` | <= 4096 B, UTF-8 (acentos; bytes que não são UTF-8 valem como Latin-1), uma célula de `6*scale` por caractere, recorte glifo a glifo |
| | `blit_rgba(ptr,w,h,dx,dy)` | 1:1, <= 2^20 px, cobra 1 de combustível por 8 px |
| | `draw_image_png(ptr,len,x,y) -> i32` | decodifica no host (`kitsune_core::png`), <= 512x512, devolve `w<<16\|h` ou erro; cobra combustível por pixel |
| entrada (exports do guest) | `on_key(code, mods)` | `mods`: bit0 Shift, bit1 Ctrl, bit2 Alt; `code`: ASCII, 10 Enter, 27 Esc, 8 Backspace, 127 Del, 0x100.. setas/Home/End/PgUp/PgDn |
| | `on_text(cp)` | caractere já traduzido pelo mapa de teclado |
| | `on_pointer(x,y,buttons)` | coordenadas do conteúdo; `buttons` bit0 esq., bit1 dir.; chamado em mudança de posição/botão |
| | `on_scroll(dx,dy)` | passos de roda (**reservado**: o driver PS/2 do kernel não decodifica a roda; o export existe na ABI e o SDK, mas nenhum evento é entregue neste build) |
| | `on_resize(w,h)` | nova área de conteúdo; seguido de `render` |
| | `on_tick(dt_ms)` | a cada `tick_ms`; `dt_ms` = ms desde o tick anterior |
| | `on_close()` | pedido de fechar; o app tem uma chamada para salvar |
| | `render()` | quando há `request_redraw`, após entrada, resize ou tick. A superfície **guarda o quadro anterior** (o host copia frente->trás antes), então o app pode desenhar só o que mudou; após um `on_resize` a superfície nova começa vazia |
| tempo | `now_ms() -> i64` | relógio de parede em ms (RTC) |
| | `monotonic_ms() -> i64` | ms desde o boot |
| | `random() -> i32` | xorshift por app, semente do relógio; **não criptográfico** |
| sistema | `lang() -> i32` | idioma da interface: 0 português do Brasil, 1 inglês (outros no futuro); pergunte de novo a cada `render`, o idioma muda com o app aberto. O SDK: `lang()` e `tr(pt, en)` |
| | `log(ptr,len)` | <= 512 B por chamada; prefixo `[app <id>]`; 16 KiB por execução, depois descartado |
| | `exit(code)` | encerramento limpo (estado `Encerrado`, sem erro) |
| clipboard | `clip_get(ptr,cap) -> len`, `clip_set(ptr,len)` | `clipboard=rw`; <= 256 B (o clipboard do SO) |
| arquivos | `fs_open(path,plen,flags) -> fd` | fd >= 1; flags: 1 READ, 2 WRITE, 4 CREATE, 8 TRUNC, 16 APPEND |
| | `fs_read(fd,ptr,len)`, `fs_write(fd,ptr,len)` | <= 64 KiB por chamada; devolve a contagem |
| | `fs_seek(fd, off: i64, whence) -> i64` | 0 SET, 1 CUR, 2 END |
| | `fs_close(fd)`, `fs_stat(path,plen,out)` | `out`: 16 B (`kind u32`, `size u64`, reservado) |
| | `fs_readdir(path,plen,index,out,cap)` | `out[0]`=tipo (1 arquivo, 2 pasta), resto = nome; devolve o tamanho do nome, 0 = fim |
| | `fs_mkdir`, `fs_unlink` (arquivo ou pasta vazia), `fs_rename(from,flen,to,tlen)` | |
| rede | `net_http_get(url,ulen,out,cap) -> i32` | `net=http`; só `http://` e `https://`, só o corpo de uma resposta 2xx (decodificado), truncado em `cap` (<= 256 KiB), tempo limite 8 s (20 s em https), filtro de destinos e `net_hosts` §7; devolve o tamanho do corpo ou `ERR_NET` (qualquer falha, status não 2xx, certificado recusado) / `ERR_PERM` (sem permissão ou fora de `net_hosts`) |

`render` é chamado só quando há motivo (app `abi=2` orientado a eventos: um app parado não
consome CPU). Um app **v1** (sem `on_*` v2) mantém o laço contínuo atual (`render` a cada
16 ms); `on_key` v1 tem um parâmetro e v2 dois: o host escolhe pela **assinatura** do export.

## 5. Arquivos: raiz por app, sandbox, descritores, cota

A decisão é toda de `kitsune_core::appfs`:

* **`AppFs`** é um trait puro e sem estado de descritor (operações por caminho absoluto já
  normalizado: `stat`, `read_at`, `write_at`, `truncate`, `create`, `mkdir`, `remove`,
  `rename`, `read_dir`, `tree_size`). `MemFs` é a implementação em memória (o oráculo dos
  testes). **`VolumeFs`** (W18) é a do kernel: um adaptador sobre o `Backend` do VFS do
  desktop, o mesmo objeto que o Arquivos usa, que serve o **disco OJFS v3** (quando montado) e
  o **volume em RAM** do fallback; `/apps/<id>.wasm`, `/data/<id>` e `/home` ficam no mesmo
  espaço de nomes do usuário e **persistem entre boots**. **Ponto único de troca no kernel:**
  `kernel/src/wasm/appfs_backend.rs::with` (o resto do código só vê `&mut dyn AppFs`).
* **Defesa em profundidade do `VolumeFs`:** só caminhos canônicos; só as três árvores da
  plataforma (`/apps`, `/data`, `/home`) são alcançáveis: `/etc`, `/var`, `/.trash` e as pastas do
  usuário dão `ERR_PERM` mesmo que o `Sandbox` falhe; `/apps`, `/data` e `/home` não podem ser
  removidos nem renomeados; um arquivo tem no máximo 64 MiB; a listagem pula nomes que o app não
  consegue endereçar (não ASCII, criados pelo Arquivos); `NoSpace` do volume vira `ERR_NOSPC`.
  Cada chamada é **uma seção crítica** do `YieldMutex` do volume (sem mascarar interrupções, sem
  reentrada: uma chamada aninhada ou um dono morto viram `ERR_INVAL`, nunca travam) e nunca
  atravessa a execução do guest; o estado do sandbox (descritores, cota) fica no `HostState` do
  app. Custo medido: cada `write` é uma transação com barreiras de flush (13 a 23 ms no QEMU
  sem KVM), por isso o SDK já divide gravações grandes em blocos de 64 KiB.
* **Normalização** (`appfs::normalize`): barras duplas e `.` somem; `..` só resolve
  lexicalmente **dentro** da raiz do guest e, se subir acima dela, é **erro** `ERR_PERM`
  (nunca "preso na raiz": um app que tenta `../../etc/x` deve saber que errou e é
  contado). Nome de componente: 1..=48 bytes ASCII imprimíveis, sem `/ \ : * ? " < > |`,
  sem controle/NUL, não só pontos, sem espaço/ponto no fim, sem bytes não ASCII; caminho
  <= 256 B e <= 8 componentes. O caminho real é `raiz_do_app + normalizado`, juntado
  **por componentes validados**, nunca por concatenação do texto do guest.
* **`SandboxFs`** envolve um `&mut dyn AppFs`: aplica a permissão (`none/own/home`), o prefixo
  (`/data/<id>` ou `/home`), a tabela de descritores (`max_fds`; fechar tudo ao encerrar o
  app), a cota (`disk_kib`: soma de tamanhos + 256 B por entrada; `write`/`truncate` além
  dela dá `ERR_NOSPC`, com escrita parcial até o limite) e o teto por chamada.
  Renomear um arquivo (ou a pasta que o contém) aberto **acompanha** o descritor: ele passa a
  apontar para o novo caminho.
* **Isolamento entre apps:** cada app só enxerga a própria raiz; `/data/<outro>` é
  inalcançável porque o prefixo é fixo por instância. A raiz `home` é compartilhada por
  definição (é o diretório do usuário).

## 6. Execução: AppManager e escalonamento

`kernel/src/wasm/` passa de "um app" para `AppManager`: uma tabela (<= 8 instâncias) de
`AppInst { Store, Instance, manifesto, quotas, estado, fila de eventos, superfície, contas }`.

**Estados:** `Iniciando` (carregando/`_initialize`) -> `Rodando` <-> `Suspenso` (janela
minimizada: sem `render`, sem ticks, eventos limitados) -> `Encerrado` (`exit` limpo) ou
`Crashou(motivo)` (trap, falta de combustível, ponteiro inválido, falha de carga). Uma
instância `Crashou` mantém a janela com **"O app encerrou: <motivo>"** até fechar; a `Store` (e
com ela a memória linear) é **destruída no ato**. Fechar a janela mata a instância; reabrir
cria uma nova.

**Escalonamento:** uma única thread `appd` (substitui `wasmapp`; o limite de 8 threads do
kernel impede uma thread por app, e uma só dona de todas as `Store` evita travas entre
apps). Laço de **round-robin por fatia**: a cada volta cada app pronto roda uma fatia (até
8 eventos e um `render`, cada chamada com seu próprio orçamento `fuel_frame`), na ordem
circular a partir do último atendido. Isso dá justiça por construção: um app não roda duas
fatias antes de os outros prontos rodarem uma. Sem nenhum app pronto, `appd` faz
`sched::block(prazo_mais_próximo, ...)`: **uma thread parada não consome CPU**; entrada,
abertura de janela e `request_redraw` fazem `sched::wake`. Como o kernel é preemptivo, uma
fatia longa não trava a UI; no pior caso atrasa os outros apps em 1 fatia (limitada pelo
combustível).

**Contabilidade por app:** combustível consumido (`fuel_total`), ticks de CPU (medidos em
torno de cada chamada com `interrupts::ticks()`), pico de memória linear, nº de chamadas.
O app Tarefas (aba Processos) mostra, por app: nome e instância, estado (Em espera, Ativo, Suspenso,
Encerrado, Parado), CPU (% do tempo de relógio das fatias do app na última janela de 1 s,
medido com o TSC; inclui preempção) e memória linear.

**Isolamento de falhas:** qualquer erro de uma chamada ao guest (`OutOfFuel`, trap,
ponteiro inválido, `memory.grow` negado que o guest transforma em trap) vira `Crashou`
**só daquela instância**; as demais, o compositor e o desktop seguem. `memory.grow` acima de
`mem_mib` é negado pelo `StoreLimits` (devolve -1 ao guest, como manda a especificação).
Tarefas: **Encerrar** (ou `Del`) fecha a janela (e mata o app) e **Reiniciar** (`R`) reinicia a instância
selecionada (descarta a `Store` e instancia de novo o mesmo pacote, na mesma janela).
Limite honesto: se a **thread** `appd` morrer (pânico dentro do `wasmi`), todos os apps
morrem juntos e a janela mostra "App encerrado"; não há reinício automático (mesma TCB de hoje).

**Janela como instância do WM dinâmico:** `Kind::WasmApp` fica (multi-instância, `multi=true`);
o estado `App::Wasm(Box<WasmWin>)` guarda o id da instância no manager. Tamanho padrão e mínimo e
`resizable` vêm do manifesto (janela = conteúdo + 28 x 56 px de moldura). A superfície
offscreen é **por instância**, no formato do framebuffer, e é **realocada no tamanho da janela**
quando ela muda (o app recebe `on_resize` e redesenha). Dois buffers (frente/trás) com troca
protegida por um trinco de uma palavra; sem quadro torto. O dock e o desktop do boot não mudam
(§8).

## 7. Rede (`net=http`)

`kitsune_core::appnet` decide, o kernel só transporta:

* esquemas `http` e `https`; porta 1..=65535; URL <= 512 B, ASCII, sem espaços/controles, sem
  `user@`;
* **destinos recusados por padrão (`ERR_NET`)**: nome `localhost`/`*.localhost`/`*.local`/
  `*.internal`; IPv4 literal em `127/8`, `10/8`, `172.16/12`, `192.168/16`, `169.254/16`,
  `0.0.0.0/8`, `100.64/10`, multicast e reservado (`>= 224`); qualquer IPv6 literal
  (`[...]`); inteiros/octais/hex disfarçados de IP (`2130706433`, `0x7f.1`). Documentado:
  o gateway do QEMU (`10.2.2.x`) e a LAN ficam fora de alcance de apps; só endereços
  públicos. Um alvo que *resolve* para endereço local não é checado pelo app (a resolução é do
  kernel) e o kernel repete a checagem sobre o IP resolvido;
* limites: corpo <= 256 KiB, 8 s de tempo total, **uma** requisição por vez por app, 1 por
  segundo; combustível cobrado por KiB recebido.

**Transporte (W18).** `net_http_get` usa a pilha real: a thread `fetcher` (dona da NIC, `netd`).
O *slot* de requisição é dividido com o navegador por um `compare_exchange` `IDLE -> CLAIMED`;
cada resultado só é retirado por quem o pediu; um app que estoura o prazo **abandona** o slot e o
`fetcher` descarta o resultado tardio. A política vale **duas vezes**: `appnet::authorize` antes de
postar (permissão, tamanho, filtro de destinos, `net_hosts`) e no `fetcher` a cada salto de
redirecionamento (um `Location` para IP privado, outro host ou `https -> http` é recusado como um
pedido direto); depois da resolução DNS o endereço **resolvido** é checado de novo
(`Net::set_public_only`), então um nome público que aponta para `127.0.0.1` ou para a LAN é
barrado. TLS é o do navegador, **com verificação completa** (cadeia, nome, `CertificateVerify`,
`docs/SECURITY-MODEL.md` §3.1) e **sem** o "continuar mesmo assim": para um app, certificado ruim
é `ERR_NET`. Só o corpo de uma resposta 2xx chega ao app (`appnet::app_response`: `chunked`,
gzip e deflate decodificados, cortado em `cap`); outro status, resposta malformada ou
codificação que não decodifica é `ERR_NET`, nunca lixo.

*Limitação honesta:* a chamada é síncrona e há **uma** thread `appd` para todos os apps, então
enquanto um app espera a rede (até 8 s em http, 20 s em https) os **outros apps** não rodam; o
compositor, o navegador e o resto do sistema seguem. Retomar a chamada de forma assíncrona
(`wasmi` resumable) é trabalho futuro. Prova: `tools/perf/scen/w18-net.sh` (TESTING.md §6).

## 8. Instalação e lançador

* Apps instalados: `/apps/<id>.wasm`. `appinstall::install(fs, bytes)`: (1) lê as seções,
  (2) valida o manifesto, (3) valida as quotas (recusa pedido acima do teto), (4) decodifica o
  ícone (recusa PNG inválido ou > 64x64), (5) recusa **id duplicado**, (6) grava
  `/apps/<id>.wasm`. `remove(id)` apaga o arquivo (e deixa `/data/<id>` intocado, a menos que
  o usuário peça "remover com dados"). Nada é instalado parcialmente (grava em nome
  temporário e renomeia).
* O catálogo (`AppCatalog`, em memória, reconstruído ao instalar/remover) guarda id, nome,
  versão, ícone decodificado em 24x24 e o manifesto. O launcher **Apps** lista os apps
  instalados abaixo dos apps do sistema, com ícone e nome, e **rola** quando são muitos (até 11
  linhas; setas Cima/Baixo/Home/End com o painel aberto, ou clique na barra à direita). O menu
  de contexto da área de trabalho continua listando só os apps do sistema.
* **Barra e boot idênticos:** a barra de tarefas continua com os 7 ícones atuais e o ícone "WASM" abre o app
  padrão (`snake`), como hoje. Como o launcher Apps só aparece aberto, o desktop do boot
  fica pixel a pixel igual à linha de base (`tools/verify-boot.sh`, 0 pixels).
* **Primeiro boot:** a imagem embute os apps `clock`, `notes`, `paint` e `snake` e `appinstall::seed_once` instala os que faltam em `/apps`, sem sobrescrever
  o que o usuário já tem. Com o volume persistente cada pacote embutido é oferecido **uma vez**:
  o arquivo `/apps/.seeded` guarda os ids já oferecidos, então um app que o usuário removeu **não
  volta** no boot seguinte, e um pacote novo numa versão posterior do sistema ainda é instalado
  (`appinstall::seed`, sem memória, continua para volumes que não persistem). Num disco em
  branco a primeira semeadura grava ~700 KiB e leva ~3 s no QEMU sem KVM, antes do primeiro
  quadro do desktop; nos boots seguintes não grava nada.
* **Gerenciador de arquivos:** o lugar **Apps** da barra lateral (tecla `A` em qualquer pasta;
  `Tab`/`Backspace` voltam) lista os pacotes instalados e os embutidos ainda não instalados, com
  ícone, tamanho do pacote e estado; `Enter` executa (instalando antes se for um pacote
  embutido), `I` instala, `Del` remove (os dados em `/data/<id>` ficam), o botão direito oferece
  Abrir/Instalar/Remover/Propriedades e uma falha aparece na linha de estado. Propriedades de um
  app, e de qualquer arquivo `.wasm`, mostram o manifesto: versão, ABI, arquivos, rede,
  área de transferência, memória/disco/descritores, janela e se está instalado. `Enter` num
  arquivo `*.wasm` valida o pacote, instala se for novo e executa. Um app que grava arquivos
  aparece no Arquivos em até 100 ms (o contador `vfs::generation` sobe e as janelas recarregam).
* `/apps`, `/data` e `/home` **persistem** no disco OJFS v3 (provado em QEMU: o Notas salva
  `nota-1.txt` em `/data/notes`, o sistema reinicia com a mesma imagem e a nota abre; o boot
  seguinte loga `apps: 0 bundled packages installed`). Sem disco v3 (sem disco, disco pequeno,
  desconhecido) vivem no volume em RAM do desktop e se perdem ao desligar, o que o Arquivos já avisa
  (verificado com `FS_SIZE=64K`); esse volume tem 4 MiB no total e os seis pacotes embutidos já
  ocupam ~1,2 MiB, então as cotas de dados dos apps ficam limitadas pelo que sobra.

## 9. SDK e apps de exemplo

`wasm-apps/sdk` (crate `no_std`, `wasm32-unknown-unknown`, workspace isolado): `sys` (imports
`osj.*` crus), wrappers seguros (`Canvas`, `File`, `Dir`, `log!`, `Error`), o macro
`manifest!("id=clock\nname=Clock\nname.pt=Relógio\n...")` que emite `#[link_section = "kitsune.manifest"]`
(e `icon!(include_bytes!(...))`), e o `panic_handler`. Apps: `clock`, `notes`
(arquivos em `/`, ou seja `/data/notes/`), `paint` (mouse; salva BMP pelo host? não: o guest
escreve um BMP de 24 bits direto com `fs_write`, formato trivial), mais `snake`
empacotados com manifesto; `hello`, `plasma` e `nettest` ficam em `wasm-apps/examples/` (compiláveis, fora da imagem).

## 10. O que vira teste

| Área | Teste |
|---|---|
| `wasmsec` | cabeçalho errado, truncado, LEB128 longo demais/não terminado, tamanho de seção > buffer, seções demais, nomes não UTF-8, duplicadas, módulo vazio; **fuzz** `app_manifest` |
| `appmanifest` | cada chave: válido/inválido/limites; duplicada, desconhecida, `x-`, CRLF, comentário, sem `=`, >64 linhas, >4 KiB; padrões; `id` hostil; quotas acima do teto (`grant` e instalador recusam); ícone > 64x64, PNG corrompido |
| `appfs` | **tabela de caminhos hostis** (`..`, `../..`, `/../`, `a/../../b`, `//`, `.`, `\`, NUL, controle, não ASCII, nomes longos, profundidade, `%2e%2e`, `....`, `/data/outro`, `~`): nenhum escapa; descritores (teto, fechar, reuso), cota (parcial, `NOSPC`), isolamento entre dois apps no mesmo `MemFs`, leitura/escrita/seek/append/trunc, readdir, rename, unlink, permissões `none/own/home`; *property test* "nenhum caminho gerado acessa fora da raiz" |
| `appnet` | tabela de destinos permitidos/recusados, IPs disfarçados, esquemas, tamanhos; `net_hosts` (exato, curinga, nomes parecidos, base do curinga fora); ordem das recusas de `authorize`; cada salto de redirecionamento passa pelo mesmo portão; resposta só 2xx, `chunked`, corte, codificação ilegível |
| `VolumeFs` (W18) | os **mesmos** passos dão os mesmos resultados no `MemFs` e no `VolumeFs`; só `/apps`, `/data`, `/home` são alcançáveis (e as pastas-raiz não saem); persistência de dados, pacotes e remoções por um remount do `Fs3<RamDisk>`; cota exata depois do remount; volume de 1 MiB cheio dá `NOSPC` e `fsck` limpo; arquivos esparsos; sequência do sandbox deixa a mesma árvore nos dois; **fuzz** `app_sandbox` também sobre `VolumeFs` |
| `appabi` | `check_range` (estouro, zero, limites exatos) |
| catálogo/instalação | instalar, duplicado, manifesto inválido, quota acima do teto, remover, seed sem sobrescrever; `seed_once`: uma vez só, app removido não volta, pacote novo entra, marcador hostil |
| Arquivos, lugar Apps | linhas, pseudo-caminho que nunca lê o volume, seleção que acompanha o app, `Enter`/`I`/`Del` e as mensagens de erro, menu sem comandos de arquivo, texto do manifesto |
| kernel (QEMU) | provas (a) a (f): 4 apps ao mesmo tempo respondendo à entrada; app hostil (laço infinito, `memory.grow`, ponteiro inválido, `open("../../etc/x")`, descritores até o teto, ler FS de outro app, `net_http_get` sem permissão) contido; instalar/remover via Arquivos; 100 aberturas/fechamentos com heap estável; CPU por app; desktop do boot idêntico |
| kernel (QEMU), W18 | persistência de app (`w18-persist-{1,2}.sh`); lugar Apps (`w18-apps.sh`, `w18-wasmfile.sh`); rede real do app contra um servidor falso (`w18-net.sh`: 200, redirecionamento válido, redirecionamento para IP privado, 404, fora de `net_hosts`, IP privado, gzip, chunked, certificado autoassinado, nome público que resolve para loopback); 30 rodadas de abrir/fechar com o disco (heap sem deriva: 1 001 312 -> 982 416 B) |

## 11. Fora do escopo desta entrega

Sockets TCP (`net=tcp` só é aceito no manifesto), chamadas de rede assíncronas (hoje um app
esperando a rede segura os outros: §7), assinatura de pacotes, atualização de versão de app
instalado, "remover com dados", WASM threads/SIMD, áudio.
