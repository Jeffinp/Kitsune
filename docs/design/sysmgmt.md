# Gerenciamento do sistema: log, monitor, configurações e notificações

Frente W14, refeita na W25: o monitor de recursos e o gerenciador de processos viraram um app só, **Tarefas**; o visualizador de log é o **Registro**; as Configurações são os **Ajustes**; a Calculadora e os banners ganharam o visual novo. A aparência e os números da W25 estão em `docs/design/ui-design.md`, seção 11, e nas seções marcadas "W25" abaixo; o resto descreve a W14 e continua valendo para a lógica.

Frente W14. Quatro ferramentas que um SO precisa, no padrão do resto do projeto: a
lógica pura e testada em `kitsune_core`, a cola de hardware no `kernel`, prova em QEMU
(BIOS e UEFI) com capturas de tela. O desktop ocioso com as configurações padrão
continua **idêntico, pixel a pixel**, à linha de base (`tools/verify-boot.sh`, 0 pixels
de diferença nos dois modos).

| Peça | Lógica pura (`kitsune_core`) | Cola (`kernel/src`) |
|---|---|---|
| Registro (W25; era o Log do sistema) | `klog` (anel, filtro, visão, `LineAsm`/`classify`, `dump_bounded`) | `klog.rs`, `desktop/apps/registro/`, `logd.rs` (gravação em disco) |
| Tarefas (W25; era o Monitor de recursos) | `sysmon` (séries, `CpuSampler`), `activity` (nomes, formatação pt-BR, interpolação, taxas, carga, pressão, tabela ordenável) | `desktop/{tarefas,kit,live}.rs`, `ata::io_bytes`, `netd::stats()` |
| Ajustes (W25; eram as Configurações) | `settings` (com `toast_secs`, `dock_zoom`, cidades de fuso), `wallpaper`, `hw::rtc` (data/hora/fuso), `keymap` (ABNT2) | `settings.rs`, `rtc.rs`, `desktop/apps/ajustes/`, `font.rs` (Latin-1) |
| Notificações | `notify` (`Toasts`) | `notify.rs`, `desktop/shell/toasts.rs` |
| Interfaces para outras frentes | `sysif` (traits) | `desktop/services/sysstore.rs` (implementações de hoje) |

Os apps novos (**Tarefas**, **Ajustes**, **Registro**) estão no overlay Apps e na Busca; a barra de apps
tem **Tarefas** e **Ajustes** (nove itens depois da W25).

## 1. Log do sistema (`klog`)

**Formato do registro** (12 bytes de cabeçalho + texto, no máximo 200 bytes):
`seq:u32 ts_ms:u32 nível:u8 origem:u8 len:u16 texto`. Níveis `TRACE < DEBUG < INFO < WARN <
ERROR < FATAL`; o timestamp é o tick do PIT em ms (resolução de 4 ms); `origem` é o slot
da thread no escalonador (o visualizador mostra o nome).

**Anel** (`LogRing<N>`, 64 KiB em BSS): registros de tamanho variável num buffer circular.
`push` nunca falha nem aloca: se faltar espaço descarta os registros **mais antigos
inteiros** (`dropped`). Leitores não percorrem o anel vivo: `copy_out` copia os bytes
usados, do mais antigo ao mais novo, para um buffer do leitor, e `records()` decodifica.

**`klog!(Info, "...")`** escreve na serial **exatamente os mesmos bytes** que
`serial_println!` escrevia (por isso `first desktop frame`, `TSC calibrated`, `died:`,
`net: `, `storage: `, `[trace]`, `KERNEL PANIC`, `FATAL EXCEPTION` continuam iguais) e grava
um registro. Além disso, **todo** `serial_println!` do kernel, inclusive o de código de
outras frentes (ATA, NIC, fetch, WASM), é espelhado no anel: `serial::write_str` chama
`klog::capture`, que remonta linhas (`LineAsm`) e classifica o nível por palavra-chave
(`classify`: `FATAL`/`KERNEL PANIC` = FATAL, ` died` = ERROR, `failed`/`fallback`/`expired`/...
= WARN, o resto INFO; as linhas `[trace]` não entram, para o relatório por segundo do
`perf-trace` não expulsar o resto). Eventos de boot, escalonador, virtio-gpu, disco e
editor foram migrados para `klog!` com nível explícito; a saída serial é a mesma.

**Segurança em ISR** (prova por leitura e por teste):

- `klog::log` formata numa pilha de 200 bytes (`FixedBuf`), escreve na UART **fora** da
  seção crítica e só depois entra em `with_ring`, que é `without_interrupts` + a
  referência única ao anel: máquina de um núcleo, então IF=0 é exclusão mútua, sem lock
  para girar e sem como uma ISR travar contra a thread que ela interrompeu.
- Nada ali chama o alocador nem pega o lock do heap. O teste
  `kitsune_core/tests/klog_noalloc.rs` instala um alocador global contador (por thread) e
  confere **0 alocações** em 100 000 rodadas de formatar + `push` + montagem de linha da
  serial + `classify` + `for_each_since` + `copy_out` (com um controle positivo que mostra
  que o contador conta). O teste `hundred_thousand_messages_never_corrupt_the_ring`
  empurra 100 000 mensagens de tamanhos variados por um anel de 64 KiB conferindo ordem,
  `seq` consecutivo e contabilidade de bytes.
- No boot real: um gancho temporário (não commitado) empurrou 100 000 mensagens pelo
  caminho do kernel e conferiu o anel com o `copy_out` (saída na serial):
  `W14 ring check: 100000 messages pushed in 868 Mcycles, 1502 records kept (seq
  98519..100020), consecutive=true, bytes=65535` (~3 µs por mensagem no QEMU/TCG).
- O **caminho de pânico/exceção fatal não depende do klog**: `crash::die` congela o
  espelho (`klog::freeze`) e escreve só pela UART, como antes.

**Registro** (W25, `desktop/apps/registro/`): tabela com hora (fonte mono), nível (etiqueta colorida: TRACE, DEBUG,
INFO, AVISO, ERRO, FATAL), origem (nome amigável da thread) e mensagem (mono); busca (digitar vai para o campo;
`Del` limpa; `Esc` limpa e, vazia, fecha), filtro de nível segmentado (Tudo, Info, Aviso, Erro; `Tab` avança),
chave **Seguir**, **Limpar** e **Salvar** (trait `LogSink`; grava `/var/log/syslog.txt` com o log filtrado, até 256 KiB, as
últimas linhas inteiras se passar disso), rolagem em pixels com deslize (roda, setas, `PgUp`/`PgDn`, `Home`/`End`) e
barra de rolagem que some. Trabalha numa cópia (snapshot) do anel, renovada a cada segundo; a janela
visível mostra no máximo as linhas que cabem, então um anel cheio (64 KiB, ~770 linhas de teste) custa o mesmo
que um vazio. O visualizador antigo (botão de nível, caixa de busca, linhas de 18 px, painel escuro) foi substituído.

## 2. Tarefas (W25; reúne o monitor de recursos e o gerenciador de processos)

Cinco abas: **CPU**, **Memória**, **Disco**, **Rede** e **Processos**. A amostragem é **uma vez por
segundo**, no tique de relógio do laço do compositor (`Desktop::sample_system`): algumas operações
inteiras, uma passada pela tabela de janelas e uma caminhada na free-list do heap. Sem a janela
aberta não há trabalho extra além disso; minimizada ou oculta, a janela não desenha nada.

- **CPU**: carga total e gráfico de 60 s (`Series`, suavizado por um filtro 1-2-1 só para desenhar), tempo
  ligado, carga média de 1/5/15 min (`activity::LoadAvg`, média exponencial da fração ocupada),
  threads e processos, o processador (CPUID) e barras por processo.
- **Memória**: heap em uso (gráfico com eixo em potências de 2), medidor de **pressão** (`Pressure`: normal
  abaixo de 60 %, atenção até 85 %, crítica acima), em uso/livre/total/pico/RAM física e a memória aproximada
  de cada app (`App::approx_bytes`; apps WASM informam o valor real).
- **Disco**: volume, barra de uso e porcentagem, usado/livre/total, arquivos e pastas (contadores de inodes do
  `statfs`), leitura e gravação por segundo com gráfico (contadores atômicos do driver ATA, `ata::io_bytes`, sem
  lock nem alocação, só bytes de transferências bem-sucedidas), totais desde o boot, modelo e tamanho do disco
  (IDENTIFY).
- **Rede**: estado do link, IP, máscara, roteador, DNS, tempo restante da concessão, taxas de recepção e envio com
  gráfico, bytes e pacotes (`netd::stats()`); erros e descartes só aparecem quando existem.
- **Processos**: PID, nome amigável (`friendly_name`: `compositor` é Interface, `fetcher` Rede (busca), `appd`
  Aplicativos, `shelld`/`shelld2` Terminal (execução), `logd` Registro, `kernel` Sistema; o nome interno fica
  na dica e na linha de detalhe), estado (Ativo, Em espera, Suspenso, Encerrado, Parado), CPU, memória e tempo ativo.
  Ordenação por qualquer coluna (segundo clique inverte; empates mantêm a ordem anterior, a lista não pula entre
  amostras), busca, seleção (pelo id estável da linha, que sobrevive a reordenar), **Encerrar** e **Reiniciar**: um
  app fecha a janela de verdade (ou reinicia a instância WASM); um serviço do sistema pede confirmação numa folha
  (Aplicativos fecha todos os apps instalados, Terminal (execução) interrompe os comandos em andamento; os demais não
  podem ser encerrados e o botão fica desativado). Rodapé: processos, threads, CPU, memória e disco.

**Movimento.** Cada amostra nova dispara 450 ms de animação: o gráfico desliza um passo (a ponta interpola entre as
duas últimas amostras), o número do topo desliza (ease-out). Só o título e o gráfico são repintados (`live_rect`,
um quadro a cada 20 ms); o resto da janela espera o quadro de acomodação do fim. Passar o mouse sobre um gráfico mostra
o valor e "há N s". Ligado a *reduzir movimento*, tudo cai no valor final no mesmo quadro.

**O que a coluna CPU significa.** Para as threads é real: o escalonador soma 1 tick em
`TICKS[slot]` só quando o tick encontra a thread *executando* (não parada em `hlt`);
`CpuSampler` transforma a variação em décimos de por cento pelo maior resto, de modo que
**threads + ocioso somam exatamente 100,0 %**. Os apps rodam todos dentro da thread do compositor, então não
têm CPU própria: a coluna mostra o **tempo de desenho** do app (TSC acumulado em
`draw_window`, em % do segundo), que é um subconjunto da linha Interface; para apps WASM é a CPU real medida
pelo gerenciador. Memória por app só existe como estimativa do estado da instância; `—` quando o kernel não acompanha.

**Lógica pura (`kitsune_core::activity`, 16 testes).** Nomes amigáveis, formatação pt-BR (`12,3%`, `1,5 MiB`, `3 min 05 s`,
`1.234.567`), `Glide`/`ease_toward` (aproximação exponencial que nunca passa do alvo e sempre termina), interpolação
fracionária e filtro 1-2-1 da história, qual amostra está sob o ponteiro, **taxas a partir de contadores com volta
ao zero** (`wrapping_delta`, `Rate`), carga média, nível de pressão, tabela ordenável estável (nomes sem acento).

## 3. Ajustes (W25; eram as Configurações, `desktop/apps/ajustes/`)

`Settings` (`kitsune_core::settings`) é um `Copy` com texto `chave=valor`:

```
# Kitsune settings
version=1
wallpaper=2                # preset 0..4, ou "image"
wallpaper_path=papel.png   # com wallpaper=image
accent=1                   # 0..7 (paleta)
clock=12                   # 12 ou 24
tz=-120                    # minutos a leste de UTC (-720..840)
keyboard=abnt2             # us | abnt2
toasts=1
appearance=auto            # auto | light | dark
reduce_motion=0
toast_secs=4               # quanto tempo um banner fica: 2..15
dock_zoom=100              # ampliação da barra de apps: 0..100 %
tz_city=17                 # cidade escolhida na lista de fusos (índice)
```

`Settings::parse` é **total**: comentários, chaves desconhecidas e valores inválidos ou
fora da faixa são ignorados (o campo fica no padrão), versão ausente ou nova não atrapalha,
`wallpaper=image` sem caminho volta ao padrão. Os padrões reproduzem o desktop de sempre.

Os Ajustes têm nove seções (Aparência, Papel de parede, Barra de apps, Teclado, Data e hora, Rede, Disco, Energia,
Sobre) com os controles do toolkit; o texto abaixo é da W14 e vale para a lógica de cada coisa. Mudanças da W25: o tema
é um controle segmentado (Automático, Claro, Escuro), a cor de destaque são oito amostras com anel de seleção, *Reduzir
movimento* e *Notificações* são chaves com a duração dos banners num controle deslizante, o papel de parede é uma grade
de miniaturas desenhadas ao vivo (aparência atual) mais uma imagem do usuário (campo de caminho e "Escolher imagem…",
que abre o Arquivos), a barra de apps tem um controle de ampliação com pré-visualização, o fuso é uma lista de 52
cidades com busca (`settings::TIMEZONES`), Rede e Disco leem ao vivo `netd::stats()` e o volume, Energia abre a folha
de confirmação e Sobre mostra versão, processador, memória, tempo ligado, tela, como iniciou e uma linha sobre a fonte
de números aleatórios. Cada página é uma função do construtor imediato `Ui` (`apps/ajustes/pages/`): desenha, acha o clique e
acha o hover pelo mesmo código. Um controle deslizante aplica ao vivo e **grava o arquivo uma vez, ao soltar**.

- **Aparência**: papel de parede (o original *Indigo*, três gradientes novos, um sólido e
  uma **imagem do usuário** por caminho de arquivo: `wallpaper::load` limita o arquivo
  (4 MiB) e as dimensões do cabeçalho PNG/BMP (8 Mpx) **antes de alocar**, decodifica com
  `image::decode` e "cobre" a tela com recorte central; arquivo ausente ou recusado mantém o
  papel atual e diz o motivo), cor de destaque (8 cores), relógio 12/24 h, notificações.
  Como os ícones do dock fazem parte do fundo em cache, trocar papel ou destaque pede
  **um** repaint do fundo (`take_bg_repaint`).
- **Hora e região**: hora local ao vivo, fuso em passos de 30 min (`rtc::now` aplica em
  minutos), editor de data e hora (`DateTime::step`) que **grava o CMOS** respeitando BCD ×
  binário, 12 × 24 h, flag PM e registrador de século, com as atualizações do RTC
  paradas (`SET`) durante a escrita.
- **Teclado**: US ou **ABNT2** (`ç`, acentos mortos ´ ` ~ ^ ¨ que se combinam com vogais e
  `n`, `Keymap::take_pending` entrega a segunda tecla de acento + letra que não combina);
  o `font.rs` ganhou as letras Latin-1 montadas a partir dos glifos ASCII.
- **Rede** (somente leitura: placa, MAC, IP/máscara/gateway/DNS/lease; a W25 tirou o botão
  "Renovar DHCP", que só respondia "indisponível"),
  **Armazenamento** (`DiskUsage`, discos IDE), **Energia** (reiniciar/desligar com segundo
  clique para confirmar; bateria n/d), **Sobre**.

**Aplicação no boot**: a ordem em `main.rs` é `storage::init()` (monta/migra/formata o OJFS v3)
-> splash (que toca enquanto o resto roda, em passos entre os quadros) -> `Desktop::new` -> `load_settings` -> pintar o fundo; as configurações só são lidas
**depois** de o volume existir, e o papel de parede por imagem também sai do volume. Sem arquivo
valem os padrões. O arquivo é escrito quando o usuário muda algo (por isso o boot padrão não
muda). **Persistência** (provada em dois boots no mesmo disco, `w18-settings-{1,2}.sh`: papel de
parede por imagem `/papel.png`, destaque violeta, relógio 12 h, fuso UTC-02:00 e teclado ABNT2
voltam sem tocar nas Configurações; `settings: loaded 120 bytes from kitsune.conf`): trait
`SettingsStore`, hoje `VfsStore` (`/etc/kitsune.conf` no volume do desktop). O caminho do papel
de parede pode ser um nome simples (`papel.png`, como o FS plano antigo guardava): vale
`/papel.png` (`settings::absolute_path`).

**Log no disco** (`kernel/src/logd.rs`). "Salvar" escreve `/var/log/syslog.txt` pela thread do
compositor (uma ação do usuário, um arquivo). Já o **log de boot** é gravado por uma thread
própria, `logd`, que dorme em `sched::block` (sem fatia de tempo) até o compositor pedir, depois
do primeiro quadro do desktop (para o log ter o boot inteiro): ela renderiza o anel
(`klog::dump_bounded`: todos os níveis, no máximo 96 KiB, as linhas mais novas vencem) e grava
`/var/log/boot.log` pelo mesmo `LogSink`; cada boot substitui o arquivo; sem volume em disco ela só
registra que pulou. Regras: nunca a partir de IRQ (só em contexto de thread), nunca segura outro
lock além do volume e nunca trava o compositor, que no máximo espera uma escrita se precisar do
disco naquele instante. Falhas de escrita de arquivos do sistema e de transferência ATA agora
deixam um WARN no log (e um toast) em vez de falhar em silêncio.

## 4. Notificações (toasts)

W25: o banner tem disco com o glifo do nível, título e até duas linhas medidas, contador, botão de fechar enquanto o
ponteiro está sobre ele e uma linha fina na base que diminui até o fim; o tempo vem dos Ajustes (`toast_secs`,
`Toasts::set_lifetime_secs`, cada banner leva o seu). Enquanto há banner o compositor pede quadros (um por segundo com
*reduzir movimento*).

`kitsune_core::notify::Toasts`: no máximo 3 visíveis (4 s cada por padrão) empilhadas abaixo do painel superior,
fila de 8, uma repetição de mensagem visível só reinicia o tempo e incrementa um contador
(`x3`), clique fecha. O compositor desenha direto no framebuffer depois do quadro
(restaurando o fundo a partir do `back`, como o HUD) **só enquanto há toast na tela ou
acabou de sair**; sem toast, o custo é duas leituras atômicas por passagem do laço.

Fontes de eventos: registros **WARN/ERROR/FATAL** do klog (`klog::take_warnings`, uma
leitura atômica quando nada novo) e a API `notify!(Info, "texto {}", x)`
(`kernel/src/notify.rs`, segura em ISR, também grava no log) para os níveis menores.
Hoje: thread morta (`died:`), lease DHCP expirado, falha de escrita no disco
(`flush_disk`), disco cheio, erro de I/O, "Encerrado: app" ao encerrar pelo monitor.
Só entram eventos **depois** do desktop pronto (o cursor de leitura começa em `klog::seq()`),
então o boot não gera toast. Desligável nas Configurações.

## 5. Traits para outras frentes (`kitsune_core::sysif`)

Cada uma tem um padrão trivial no core e uma implementação do kernel com o que existe
hoje; a frente que trouxer algo melhor só implementa o trait e troca o objeto.

| Trait | Implementação de hoje | O que a outra frente liga |
|---|---|---|
| `DiskUsage` (`label`, `usage() -> DiskUsageInfo{total,used,items}`) | `VfsUsage` (`desktop/services/sysstore.rs`): capacidade real do volume (`statfs`), "OJFS v3 (IDE)" ou "Memoria" | inodes livres (`items_*`) |
| `NetStats` (`counters() -> Option<NetCounters>`) | `KernelNetStats` sobre `netd::stats()` (os contadores de `nic::STATS`, alimentados por **todos** os drivers; a página Rede das Configurações lê o mesmo `Snapshot`) | `NetCounters` pode ganhar campos (descartes, erros) |
| `NetControl` (`renew_dhcp()`) | `NoNetControl` (sempre `Unsupported`) | renovação DHCP (a W25 tirou o botão até existir uma API) |
| `LogSink` (`write_file(nome, dados)`) | `VfsSink`: `/var/log/<nome>` no volume; mantém as últimas linhas inteiras que cabem em 256 KiB (`SinkError::Truncated`) | rotação de logs |
| `SettingsStore` (`load`/`save`) | `VfsStore`: `/etc/kitsune.conf` no volume | — |

Outros pontos de integração: `klog!`/`notify!` já podem ser usados em qualquer módulo
(`netstack`, `fetch`, `ata`, `wasm` seguem com `serial_println!`, espelhado no log como INFO
ou pela palavra-chave; trocar por `klog!(Warn, ...)` dá o nível exato);
`Desktop::set_network(nic, NetConfig)` guarda a identidade de rede para a página Rede
(chamar de novo quando houver uma renovação); `Kind`/`App` em `desktop/windows/instance.rs` têm
`TaskMgr` (Tarefas), `Settings` (Ajustes) e `LogViewer` (Registro); o `Monitor` deixou de existir na W25.

## 6. Como reproduzir as provas

Todos os cenários são `tools/perf/scen/w14-*.sh` (rodam com `tools/perf/run.sh`; o de
configurações usa `FS_IMG=` com um disco que tem `papel.png`, preparado por
`cargo run -p kitsune_core --example fs3_inject -- disco.img papel.png /papel.png`; os de W18 são
`w18-*.sh`).

| Cenário | Mostra |
|---|---|
| `w14-log.sh` | eventos de boot no visualizador, busca "net", filtro WARN+ |
| `w14-mon.sh` (com gancho de carga) | processos ordenados, gráficos mudando, aba Sistema (W14; a W25 usa `w25-tarefas*.sh`) |
| `w14-endtask.sh` | `DEL` no monitor fecha a calculadora |
| `w25-tarefas.sh`, `w25-tarefas2.sh`, `w25-tarefas3.sh` | todas as abas em claro e escuro, gráficos com valor ao passar o mouse, busca, ordenação, folha de confirmação, 32 processos com roda, troca rápida de abas, tamanho mínimo |
| `w25-perf.sh`, `w25-perf-hidden.sh` | custo de uma janela Tarefas visível e minimizada (`perf-trace`) |
| `w25-log.sh` | Registro: busca, níveis, seguir, rolagem, Salvar e Limpar (com um gancho temporário que enche o anel além de 64 KiB) |
| `w25-settings.sh`, `w25-calc.sh` | todas as seções dos Ajustes em claro e escuro com as interações; teclado e mouse na Calculadora |
| `w14-set.sh`, `w14-pages.sh` | papel de parede (gradiente e PNG), destaque, 12 h, fuso, data/hora, ABNT2, páginas Rede/Armazenamento/Energia/Sobre |
| `w14-toast.sh` (com gancho) | toast WARN, expira em 4 s, toast ERROR, clique fecha |
| `w18-settings-1.sh` / `-2.sh` | W18: configurações (imagem, destaque, 12 h, fuso, ABNT2) e log salvo no boot 1; tudo de volta no boot 2 com o mesmo disco; `/var/log/{boot,syslog}.log` e `/etc/kitsune.conf` no disco (`fs3_inject --ls`) |

Os ganchos de carga e de eventos são **temporários** (não estão no repositório): uma thread
que gira 6 s e dorme 6 s, uma onda de heap e de tráfego DHCP por segundo, `klog!(Warn)`
aos 8 s e `klog!(Error)` aos 14 s, e a inundação de 100 000 mensagens.

## 7. Custo

`perf-trace`, UEFI, QEMU/TCG sem KVM, 2 execuções intercaladas por build
(`tools/perf/ab.sh`, `agg.py`; **A** = `e5257f7`, antes da frente; **B** = depois), a
máquina compartilhada com outras frentes (ruído alto; compare as medianas e os mínimos).
Tempo de parede por quadro, em µs:

| Caminho | A (antes) | B (depois) |
|---|---|---|
| ocioso, tique do relógio (`ClockLocal`) | 226 (mín 189) | 280 (mín 179) |
| ocioso, CPU ocupada | 0,83 % | 0,67 % |
| arrastar: quadro de dano (`AnimDamage`) | 16,9 ms | 24,2 ms (n=7 contra 11 quadros) |
| arrastar: só o cursor | 36 | 33 |
| arrastar: `Steady` / `Settle` | 7,0 / 12,9 ms | 5,9 / 10,4 ms |
| abrir/fechar a calculadora: `AnimDamage` / `AnimRebuild` | 14,8 / 26,5 ms | 13,5 / 24,6 ms |
| abrir/fechar: `Settle` | 10,8 ms | 10,2 ms |
| CPU ocupada, abrir/fechar | 5,39 % | 5,42 % |
| Tarefas aberto, tique do relógio (`Clock`) | 5,7 ms | 4,8 ms |

Nada regrediu além do ruído. O desktop ocioso ainda faz **só** o repaint local do relógio
por segundo; o custo novo por segundo no laço (amostragem do monitor, duas leituras
atômicas dos toasts, uma do log) não aparece no tempo de quadro e some na porcentagem de
CPU ocupada (0,7 a 0,8 % nas duas versões). A caixa de relógio 12/24 h e o fuso em minutos
usam um átomo e um `format_clock` por repaint.

Com o **Monitor aberto** a janela é "viva" como a Tarefas: o tique de 1 s recompõe
a cena e a repinta (`Clock`, **9,5 ms** por segundo, ~1 % da CPU; a Tarefas custa 4,8
ms por ser menor). Janelas vivas: Tarefas, Monitor, Configurações e Log.
`tools/perf/scen/w14-perfmon.sh` reproduz a medida.

## 8. Fora do escopo / limitações

- Sem disco v3 (sem disco, disco pequeno, desconhecido) as configurações e os logs ficam no
  volume em RAM do desktop e não sobrevivem ao desligamento (o Arquivos já avisa); o log de boot
  é pulado nesse caso. `boot.log` guarda só o boot atual (sem rotação).
- A migração `serial_println!` → `klog!` com nível explícito foi feita nos arquivos que esta
  frente pode tocar; `ata`, `netstack`, `ne2000`, `fetch` e `wasm` ficam com o espelho
  automático (INFO / palavra-chave).
- Renovar DHCP e a bateria não aparecem: não há API de renovação nem ACPI (a W14 tinha botões que
  respondiam "indisponível" e "n/d").
- A roda do mouse rola o Registro, os Ajustes e a tabela de Processos (a W17 ligou a roda).
- O lease DHCP "novo" só dispara toast quando houver renovação (a do boot acontece antes
  do desktop existir e não deve mudar o desktop ocioso).

## 9. Capturas

`docs/img/sys-*.png` (QEMU BIOS; os cenários rodam também em UEFI, conferido):

| Imagem | O que mostra |
|---|---|
| `sys-log.png` | visualizador com os eventos de boot (nível, thread, mensagem) |
| `sys-monitor.png` | aba Desempenho com carga gerada: CPU em degraus (100 % + ocioso 0 %), heap em dente de serra, rede, disco |
| `sys-settings.png` | papel de parede vindo de um PNG no disco, destaque violeta, relógio 12 h |
| `sys-keyboard.png` | ABNT2: `ç á ã õ ~ é` digitados com as teclas mortas |
| `sys-toast.png` | toast de AVISO no canto, acima do relógio |
