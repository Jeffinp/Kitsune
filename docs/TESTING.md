# Testes e verificação

O Kitsune tem cinco camadas de verificação. Cada uma responde a uma pergunta
diferente, e a lista abaixo diz **o que cada uma não cobre**.

| Camada | Pergunta que responde | Comando | Cobre | Não cobre |
|---|---|---|---|---|
| Testes unitários | A lógica pura está certa? | `cargo test-core` | `kitsune_core` (shell, editor2, calc, janelas, heap, FS v2/v3, blockdev/blockcache, rede, HTML/CSS, browser) | `kernel/` (hardware) |
| Fuzzing | Dado hostil derruba o parser? | `cd fuzz && cargo fuzz run <alvo>` | `net` (+ lease DHCP, DNS, ICMP), `fs` (v2 e v3), `web`, `shell` (com sessão de terminal), `editor2` (com os diálogos), `image`, `x509` (cadeia de certificados) | TCP/TLS/DNS (`smoltcp`, `embedded-tls`), drivers |

| Fuzzing | Dado hostil derruba o parser? | `cd fuzz && cargo fuzz run <alvo>` | `net`, `fs` (v2 e v3), `web`, `shell`, `editor2`, manifesto de app (`app_manifest`), sandbox de arquivos (`app_sandbox`) | TCP/TLS/DNS (`smoltcp`, `embedded-tls`), drivers |
| Fuzzing | Dado hostil derruba o parser? | `cd fuzz && cargo fuzz run <alvo>` | `net` (+ lease DHCP, DNS, ICMP), `fs` (v2 e v3), `web`, `html_img_form` (imagens, formulários, busca, favoritos), `shell`, `editor2`, `image`, `x509` (cadeia de certificados) | TCP/TLS/DNS (`smoltcp`, `embedded-tls`), drivers |
| Boot em QEMU | O kernel sobe e o desktop é o mesmo? | `tools/verify-boot.sh` | BIOS e UEFI, panic/exceção na serial, imagem do desktop | Hardware real, rede real |
| Lint | Há `unsafe` sem justificativa, avisos? | `cargo lint-kernel`, `cargo lint-host` | Todo o código | Corretude |
| Supply chain | Dependência vulnerável ou de licença ruim? | `cargo deny check`, `cargo audit` | `Cargo.lock` | Código das dependências |

Tudo roda no CI (`.github/workflows/ci.yml`), exceto os boots em QEMU e o fuzzing,
que precisam de mais tempo e de OVMF.

## 1. Testes unitários (`kitsune_core`)

```bash
cargo test-core          # alias de `cargo test -p kitsune_core`
cargo test -p kitsune-userabi   # o contrato de arquivos e argumentos dos programas ring 3
```

Por que só o core: um binário `no_std`/`no_main` não tem harness de teste. Por isso
toda decisão que não precisa tocar hardware mora em `kitsune_core`, que compila
com `std` sob teste e `no_std` em produção (`#![forbid(unsafe_code)]`).
Regra do projeto: **lógica nova vai para o core com teste; o kernel só liga o
hardware a ela.**

### Onde ficam os testes (Where tests live)

- **Ao lado do módulo.** Os testes do módulo `foo` ficam em `foo/tests.rs` (módulo `foo.rs`) ou
  `tests.rs` na mesma pasta (módulo `foo/mod.rs`), declarados com `#[cfg(test)] mod tests;`.
  Nenhum módulo carrega o bloco de testes inline.
- **Por assunto.** Quando os testes passam de ~500 linhas, `tests.rs` vira a pasta `tests/`: `mod.rs`
  com os auxiliares do módulo e um arquivo por assunto (`apps/editor2/tests/{movement,undo_redo,...}.rs`).
- **Auxiliares compartilhados** entre módulos ficam num lugar só: `kitsune_core/src/testutil.rs`
  (`cfg(test)`): decodificação hex dos vetores de codec e o localizador de módulos só de teste.
- **Testes de estrutura:** `kitsune_core/src/structure.rs` confere o grafo de dependências entre as
  pastas (`docs/design/code-structure.md`); `i18n/audit.rs` confere o catálogo contra as fontes.
- **Integração:** `kitsune_core/tests/` (ex.: `klog_noalloc.rs`, `golden/`); fuzz em `fuzz/`.
- **Kernel:** sem harness; a prova é `tools/verify-boot.sh` e os cenários de `tools/perf/scen/`.

Rodar uma área só (o nome do teste segue o caminho do módulo):

```bash
cargo test -p kitsune_core storage::          # OJFS, VFS, blockdev, cache
cargo test -p kitsune_core network::           # pilha de rede, DHCP, DNS, X.509
cargo test -p kitsune_core browsing::          # HTML/CSS, navegador
cargo test -p kitsune_core format::            # codecs (png, inflate, ...)
cargo test -p kitsune_core apps::editor2::     # um app
cargo test -p kitsune_core structure::         # regras de dependência
cargo test -p kitsune_core i18n                # catálogo e auditoria
```

### Cobertura

```bash
cargo install cargo-llvm-cov
cargo llvm-cov -p kitsune_core --summary-only            # linhas, funções, regiões
cargo llvm-cov -p kitsune_core --branch --summary-only   # + branches (precisa nightly)
```

Leia a tabela com cuidado: a coluna **Cover** à esquerda é de *regiões*; a de
*linhas* é a segunda. Os números de linhas incluem os próprios módulos `#[cfg(test)]`,
então a cobertura de **código de produção** é menor (~88% na auditoria). O CI
falha abaixo de 90% de linhas.

### Idiomas (`cargo test -p kitsune_core i18n`)

Confere os dois catálogos (mesmas chaves e marcadores, plurais completos), as chaves usadas em
`kernel/` e `kitsune_core/` (nenhuma faltando, nenhuma sobrando), os acentos do português
(`tools/i18n/accents.txt`, ~190 palavras e sufixos) nos catálogos e nos literais dos arquivos já migrados,
a cobertura de glifos das quatro fontes e os formatadores (números, tamanhos, datas, plurais).
`cargo test -p kitsune_core i18n_report -- --ignored --nocapture` lista os literais sem acento do resto da
árvore; `python3 -I tools/i18n-audit.py --check` confere `docs/design/i18n-audit.md`.

Cenários de tela dos apps migrados (W30; mostram o português, trocam o idioma em *Ajustes* com a janela
aberta e fotografam o inglês; `FS_IMG` como acima, com `/etc`, `/vazia`, `/Projetos` e `/leiame.txt`;
`FS_IMG=<disco.img> QEMU_MEM=512M tools/perf/run.sh <img> uefi <saida> 380 tools/perf/scen/<cen>`):

| Cenário | O que faz |
|---|---|
| `w30-files.sh` | Arquivos: lista, seleção, informações (arquivo, várias, app), menus, ordenar, ícones, pré-visualização, Apps, Lixeira vazia, pasta vazia, busca sem resultado |
| `w30-files2.sh` | Arquivos: formato não suportado, copiado, folha de cópia, lixeira com item e menu, confirmações, cancelado, excluído, "não é possível colar na lixeira" |
| `w30-viewer.sh` | Imagens: barra, painel de informações, folha de salvar e o erro "já existe", salvo, imagem inválida, janela vazia |
| `w30-editor.sh` | Editor: barra de estado, buscar sem resultado, recomeçou, substituir, linha inválida, Abrir, Salvar como, pergunta de sobrescrever, pergunta ao fechar |
Testes que dependem do idioma fixam o idioma **da própria thread** com `i18n::testlang::LangGuard::new(Lang::En)`
(sobreposição por thread em `cfg(test)`, restaurada ao sair; não mexe no átomo global, então roda em paralelo).
Só os testes da troca global de idioma usam `set_lang` com `LANG_LOCK`. O shell tem `shell::tests::i18n` (mesmas
mensagens nos dois idiomas, mesmo script com saída e status idênticos). No QEMU, `tools/perf/scen/w31-term.sh`
(Terminal) e `w31-apps.sh` (Tarefas em todas as abas, Registro, Calculadora) rodam em português, trocam em Ajustes
e repetem em inglês (`QEMU_MEM=512M tools/perf/run.sh <img> uefi <saida> 240 <cenário>`, UEFI 1280x800).
No QEMU (UEFI, 1280x800), cada app migrado na W32 tem um cenário que fotografa os dois idiomas, com a troca
feita ao vivo em Ajustes: `w32-settings.sh` (as dez páginas de Ajustes), `w32-web.sh` (página inicial,
`kitsune://sobre|favoritos|historico`, abas, busca, menu, erro de certificado, conexão recusada; precisa de
`tools/nettest-server.py` e de uma página na porta 8080 que registre o `Accept-Language`, e de
`QEMU_NETDEV="user,id=n0,net=203.0.113.0/24,host=203.0.113.5,dhcpstart=203.0.113.15,dns=203.0.113.3"`),
`w32-apps.sh` (Apps com os nomes localizados, janelas de apps, lugar Apps de Arquivos, galeria de componentes)
e `w32-toast.sh` (avisos com texto do catálogo; precisa de um gancho temporário na construção, nunca
commitado, que chama `notify::notify_key` uma vez por idioma). Conferido nos dois idiomas: o cabeçalho
`Accept-Language` chega como `pt-BR,pt;q=0.9,en;q=0.8` e como `en;q=1`; a página de erro, as páginas internas e
o menu de contexto aparecem no idioma novo logo após a troca; os títulos das janelas dos apps WASM trocam
(`Relógio` para `Clock`).

## 2. Fuzzing

Alvos em `fuzz/fuzz_targets/` (crate independente, fora do workspace):

| Alvo | Entrada | Exercita |
|---|---|---|
| `net_parse` | bytes como frame Ethernet; os mesmos bytes como mensagem DNS e como programa da máquina de lease | `kitsune_core::net` (ARP, IPv4, ICMP, UDP, DHCP, `respond`) com buffers de saída de vários tamanhos; `lease` (ticks e respostas em qualquer ordem: configuração só com lease em mãos); `dns` (resposta, cache, `Resolve` terminando dentro do limite); `icmp` (eventos, ping, `next_hop`, checksums do pedido) |
| `ojfs_parse` | bytes como imagem de disco | todas as operações do OJFS v2 (`list/read/write/remove/mkdir/trash/purge`) |
| `ojfs3_parse` | remendos sobre um OJFS v3 válido (com todos os CRC refeitos, para passar do checksum), bytes crus, ou dispositivo de tamanho qualquer | `detect`, `mount`, caminhada (`readdir/stat/read_at/path_of/trash_list`), `fsck`, 16 operações, `fsck` de novo (um FS são continua são) |
| `ojfs3_ops` | sequência de operações, com queda de energia opcional (em ordem ou cache volátil) | escrita lida de volta, `fsck` limpo, remount idêntico, estado exatamente antes/depois da operação cortada |
| `web_parse` | bytes como HTML/CSS/URL/resposta HTTP | parser, CSS, layout, `dechunk`, URL |
| `shell_parse` | bytes como linha/script de shell | lexer/parser, executor (FS em memória com limites, `SysInfo` mock com DNS, página e Ctrl+C), editor de linha com Tab, e uma **sessão de terminal** inteira (`Term`: histórico, rolagem, Ctrl+C, colar) numa janela de tamanho aleatório: o que seria desenhado sempre cabe na janela |
| `editor_ops` | documento + sequência de operações (`arbitrary`) | `editor2`: teclas, mouse, busca/substituição, undo/redo, wrap; invariantes depois de cada operação |
| `editor_dialog` | listagem de pasta arbitrária (nomes com `..`, `/`, controles) + teclas, cliques, roda, tamanhos de janela, erros e a pergunta de substituir | `editor2::dialog`: seleção, rolagem e cursor dentro dos limites, campo até `MAX_FIELD`, e todo caminho que o seletor entrega ao kernel é absoluto e normalizado; `CloseAsk` com teclas quaisquer |
| `image_decode` | bytes como PNG/BMP/PPM/zlib (cru, com CRCs reparados, PNG sintetizado ou BMP com offset ajustado) | `image::decode`, `inflate` (com `max_output` pequeno, também em fluxo), e as operações sobre a imagem decodificada (resize, fit, rotação, composição) mais a ida e volta exata dos codificadores PNG/BMP |
| `html_img_form` | `[modo, zoom, largura, ...bytes]`: os bytes como HTML cru, dentro de `<img src/alt/width>`, de um `<form>` (action, name, value, size, método) ou como payload `data:image/png;base64,` | `web::Doc` com zoom 50-300% e qualquer estado de imagem (invariantes de geometria, índices de links, imagens e campos), `FormState` (teclas, Tab, foco, colagem, query, `target`, teclas mortas), `Page::find`/`select`, `base64`, `decode_for_page`/`decode_data_uri`/`image_key`, `ImageCache` (sequências de operações dentro do teto de bytes) e o `Browser` (barra, histórico, favoritos, sugestões, páginas `kitsune://`); roda numa thread de pilha pequena |
| `x509_parse` | `[modo, bytes]`: o leitor DER/X.509 estrito, o casamento de nomes (SAN/curinga), o parser de datas, a validação de cadeia (`rustls-webpki` com âncora real) com os bytes como cadeia de 1 a 4 certificados, como folha ou intermediária substituindo as de uma cadeia de teste válida (chega à checagem de assinatura), e o `CertificateVerify` do TLS 1.3 com os bytes como assinatura | `kitsune_core::x509`, `kitsune_core::tlsverify` (nunca pânico nem travamento; semente: os certificados de `tools/gen-test-certs.py`) |

| `http_body` | `[modo, corte(2 B), ...bytes]`: os bytes como resposta HTTP inteira, como corpo sob cabeçalhos hostis (`gzip, gzip, deflate`, `br`, `Content-Length` enorme...), em leitura parcial do `Inflater`, e (modo `0x10`) comprimidos por nosso codificador (gzip, zlib, deflate cru, gzip em cadeia, `chunked`), **cortados** em qualquer ponto | `browser::body_partial`/`page_body_partial`/`body_bytes`, `appnet::app_response`, `Inflater::read_partial`. Invariantes: nunca pânico, corpo <= `MAX_DECODED_BYTES`, um fluxo válido cortado decodifica para **prefixo do original** e inteiro decodifica exato e sem aviso (regressões: `fuzz/regressions/http_body/`) |
| `app_manifest` | bytes como `.wasm` inteiro, como payload de `kitsune.manifest` ou de `kitsune.icon` (embrulhado numa seção válida), ou como manifesto/ícone soltos | `wasmsec` (cabeçalho, seções, LEB128), `appmanifest` (chaves, quotas, `net_hosts`, ícone PNG até 64x64) e as invariantes do manifesto aceito (inclui: `net_hosts` limitado, só com permissão de rede, nunca admite o que o filtro de destinos recusa) |
| `compositor_ops` | sequência de operações de área de trabalho (abrir, fechar, mover, redimensionar, focar, minimizar, encaixar, áreas de trabalho, overlays, ticks de animação, janelas vivas, relógio, quadros ociosos), 1 a 4 por quadro | `kitsune_core::compositor`: depois de cada quadro o resultado incremental é idêntico ao redesenho completo, o pintor nunca escreve fora do footprint, um quadro ocioso não planeja nada. 5 minutos: 25 351 execuções (84/s, até 400 quadros cada), 0 falhas |
| `i18n_format` | `[modo, idioma, ...bytes]`: modelos de mensagem hostis (chaves desbalanceadas, nomes enormes, índices posicionais), números/tamanhos crus, datas fora de faixa, chaves e etiquetas de idioma arbitrárias | `kitsune_core::i18n`: nunca pânico, saída limitada, números só com dígitos e separadores, todo texto do catálogo formata com argumentos hostis |
| `security_db` | texto como banco de contas (`parse` e ida e volta), modos e credenciais para as regras de permissão, hashes de senha malformados | `kitsune_core::security`: o que lê tem de reescrever igual; root nunca perde leitura e escrita; `password::verify` não entra em pânico |
| `userabi` | caminhos de programas ring 3 (`resolve`), blobs de argumentos (`pack_args`/`unpack_args`) arbitrários | `kitsune-userabi`: caminho normalizado, absoluto, idempotente, sem `.`/`..`; nada além do blob; ida e volta dos argumentos |
| `archive_text` | bytes como arquivo ZIP (listar e extrair com teto), nomes de entrada e conteúdo reescritos por `ZipWriter` e relidos, e bytes como texto em qualquer codificação | `format::zip`, `format::text`: nunca pânico, nada extraído passa do teto, nomes seguros nunca saem da pasta, o que escrevemos lemos igual, Windows-1252 volta idêntico |
| `entropy_api` | sequência de operações (`add` com id e crédito declarado quaisquer, timestamps, `fill` de qualquer tamanho, reseed, relógio) | `kitsune_core::entropy`: nunca pânico; crédito por fonte <= 8 bits por byte e nunca decrescente; nota nunca decrescente e timing sozinho nunca Strong; crédito na chave <= 256 por classe; nenhuma saída de 32 bytes se repete |
| `webm_demux` | `[modo, pedaço, busca, ...bytes]`: os bytes crus, ou dentro de um segmento de tamanho desconhecido (com ou sem uma lista de trilhas válida VP9 + Opus), empurrados em pedaços de 1 a 256 bytes, com uma busca (`seek`) no meio | `kitsune_core::webm`: nunca pânico; um pedaço só e pedaços pequenos dão os mesmos eventos até o primeiro erro; pacote até `MAX_BLOCK`, trilhas até `MAX_TRACKS`, cues até `MAX_CUES` e em ordem |
| `app_sandbox` | sequência de operações com caminhos em bytes crus sobre dois apps que dividem um `MemFs` **ou** um `VolumeFs` sobre um OJFS v3 de 1 MiB em RAM (o primeiro bool escolhe; mais URLs) | `appfs` (normalização, `Sandbox`, `VolumeFs`, cota, descritores) e `appnet`: nada existe fora de `/data/<id>`, os arquivos do sistema e do usuário ficam intactos, a cota vale, `fsck` limpo, URL aceita nunca é local |

```bash
cargo install cargo-fuzz
cd fuzz
cargo fuzz run net_parse -- -max_total_time=600 -print_final_stats=1 -dict=dict/net.dict
cargo fuzz run ojfs_parse -- -max_total_time=600
cargo fuzz run ojfs3_parse -- -max_total_time=600
cargo fuzz run ojfs3_ops   -- -max_total_time=600
cargo fuzz run web_parse  -- -max_total_time=600 -dict=dict/web.dict
cargo fuzz run shell_parse -- -max_total_time=600 -print_final_stats=1 -dict=dict/shell.dict
cargo fuzz run editor_ops  -- -max_total_time=600 -print_final_stats=1
cargo fuzz run editor_dialog -- -max_total_time=600 -print_final_stats=1
cargo fuzz run image_decode -- -max_total_time=600 -print_final_stats=1 -dict=dict/image.dict
cargo fuzz run x509_parse -- -max_total_time=600 -print_final_stats=1
cargo fuzz run http_body -- -max_total_time=600 -print_final_stats=1

cargo fuzz run app_manifest -- -max_total_time=600 -print_final_stats=1
cargo fuzz run app_sandbox -- -max_total_time=600 -print_final_stats=1
cargo fuzz run i18n_format -- -max_total_time=600 -print_final_stats=1
cargo fuzz run entropy_api -- -max_total_time=600 -print_final_stats=1
cargo fuzz run compositor_ops -- -max_total_time=300 -print_final_stats=1
cargo fuzz run html_img_form -- -max_total_time=600 -print_final_stats=1
cargo fuzz run webm_demux -- -max_total_time=600 -print_final_stats=1
```

O perfil de release do `fuzz/` liga `overflow-checks` e `debug-assertions`: um
estouro aritmético que no kernel (release, sem checagem) daria a volta em silêncio
aqui vira crash.

**Regressões.** Cada bug achado ficou como arquivo em `fuzz/regressions/<alvo>/`
(entrada mínima) **e** como teste unitário em `kitsune_core`. Para repetir:

```bash
cd fuzz
cargo fuzz run web_parse regressions/web_parse/html-nesting-stack-overflow
```

Resultado da auditoria (10 minutos por alvo, em paralelo, QEMU não envolvido):

| Alvo | Execuções | execs/s | Cobertura do código-alvo |
|---|---|---|---|
| `net_parse` | 38,2 M | 63 mil | `net.rs` 99,35% das linhas |
| `ojfs_parse` | 1,0 M | 1,7 mil | `fs.rs` 97,25% |
| `web_parse` | 0,69 M | 0,4–0,6 mil | layout/style 100%, dom 99%, css 98,6%, `browser.rs` 63,6% |
| `image_decode` | 0,30 M (10 min, 4 núcleos compartilhados) | 0,5 mil | `bmp.rs` 94,9% das linhas, `png.rs` 93,9%, `ppm.rs` 88,6%, `image.rs` 83,6%, `inflate.rs` 82,6%, `deflate.rs` 82,0% (o restante é `Display`, ramos de alocação falha e erros só alcançáveis por teste unitário) |

`web_parse` ainda ganhava cobertura no fim: rode por horas antes de confiar nele.

Alvos do editor v2 e do shell (10 minutos cada, rodando em paralelo, corpus vazio no
início, nenhum crash; cobertura de regiões/linhas por `cargo fuzz coverage` sobre o
corpus final):

| Alvo | Execuções | execs/s | Cobertura libFuzzer (arestas) | Cobertura do código-alvo |
|---|---|---|---|---|
| `shell_parse` | 570 mil | 950 | 6382 | `parse.rs` 96% das linhas, `line.rs` 93%, `glob.rs` 88%, `fs.rs` (MemFs) 87%, `exec.rs` 77%, `builtins.rs` 65% |
| `editor_ops` | 1,62 M | 2690 | 2095 | `editor2` regiões: `view.rs` 97%, `keys.rs` 95%, `buffer.rs` 92%, `mod.rs` 90%, `search.rs` 84%, `undo.rs` 72% |

O primeiro `editor_ops` achou um erro **da harness** (assumia que refazer tudo devolvia o
texto final mesmo com refazeres pendentes de operações `Undo` anteriores), corrigido na
harness: não houve bug na biblioteca. Os trechos que o fuzz não alcança (limite de
memória do desfazer, comandos que dependem de `SysInfo` real) são cobertos pelos testes
unitários.

**OJFS v3** (10 minutos por alvo, rodados em paralelo numa máquina de 4 núcleos, sem
crash em nenhum dos dois; nenhuma regressão a registrar em `fuzz/regressions/ojfs3_*`):

| Alvo | Execuções | execs/s | Cobertura (libFuzzer) | Linhas de `fs3` cobertas pelo corpus (`cargo fuzz coverage`) |
|---|---|---|---|---|
| `ojfs3_parse` | 114.724 | 190 | 3259 arestas, 763 entradas no corpus | `dir.rs` 93,7%, `inode.rs` 98,3%, `layout.rs` 93,2%, `bits.rs` 94,0%, `extent.rs` 83,2%, `mod.rs` 79,9%, `ops.rs` 77,5%, `fsck.rs` 76,4% (`migrate.rs` fora do alvo: coberto pelos testes) |
| `ojfs3_ops` | 196.201 | 326 | 3764 arestas, 1359 entradas | (alvo de sequência e de queda de energia; não medido por linha) |

O `ojfs3_parse` refaz os CRC depois dos remendos (`fix_crc`): sem isso o fuzzer não
passa do checksum e só exercita a rejeição. Como o `fsck` é a prova de consistência,
ambos terminam com `fsck`.

### OJFS v3: queda de energia, desempenho

```bash
cargo test -p kitsune_core fs3::tests::crash         # corta a energia em cada setor/flush
cargo test -p kitsune_core -- --ignored many_seeds   # 60 sementes x 6000 operações vs. modelo
cargo run --release -p kitsune_core --example ojfs3_bench   # MiB/s e setores por operação
```

`cargo test -p kitsune_core` compila o core com `opt-level = 2` (perfil `test` no
`Cargo.toml` raiz): as varreduras de queda de energia rodam em segundos em vez de minutos.
O kernel e os perfis `dev`/`release` não mudam.

### Injetar arquivos num disco v3 (host)

`kitsune_core/examples/fs3_inject.rs` abre (ou formata, se estiver em branco, ou migra, se for
v2) a imagem do disco do sistema de arquivos e copia arquivos para ela, para testar o desktop
com dados grandes sem digitá-los:

```bash
truncate -s 64M fs.img
cargo run --release -p kitsune_core --example fs3_inject -- fs.img foto.png /Imagens/foto.png
cargo run --release -p kitsune_core --example fs3_inject -- fs.img --files 2000 /many   # 2000 arquivos
cargo run --release -p kitsune_core --example fs3_inject -- fs.img --ls /
KEEP_FS=1 tools/qemu-headless.sh bios out 25      # usa o <out>/fs.img existente
```

Cria as pastas que faltam, recusa uma imagem com conteúdo desconhecido (nunca reformata) e
copia em blocos de 1 MiB (arquivos maiores que o limite de 64 MiB de `read_file` também
entram). Teste de ponta a ponta do gerenciador: copiar/mover/renomear/apagar/restaurar um
arquivo de 3 MB, reiniciar com `KEEP_FS=1` e conferir que tudo persistiu e que o log do
`storage` diz `fsck clean`.

Cenários do gerenciador de arquivos e do visualizador (`tools/perf/scen/w15a-*.sh`; precisam de
um disco preparado com o `fs3_inject`: `/big.bin` de 3 MB, `/Imagens/{foto.png,foto.bmp,foto.ppm,
transparente.png,corrompida.png,pequena.png}` e `--files 2000 /many`):

| Cenário | O que faz |
|---|---|
| `w15a-viewer.sh` | abre o Arquivos, entra em `/Imagens`, abre a imagem corrompida (erro na janela), BMP, zoom, 100 %, girar, painel de informações, PNG, PPM, PNG transparente |
| `w15a-files-ops.sh` | copia o arquivo de 3 MB entre pastas (barra de progresso), renomeia (F2), recorta/cola, apaga para a lixeira, restaura, exclusão permanente com confirmação |
| `w15a-many.sh` | abre a pasta de 2000 arquivos e rola (PageUp/PageDown/setas/Home/End); com `perf-trace`, os tempos de quadro saem no serial (medido: quadro estável 14 a 31 ms no TCG) |
| `w15a-soak.sh` | 100x abre/fecha o Arquivos e 100x abre/fecha o Visualizador; `tools/perf/w8-heap.sh <serial.log>` mede a deriva do heap (medido: +1544 B em 200 ciclos) |

Cenários do Editor e do Terminal (W15b, `tools/perf/scen/w15b-*.sh`; disco em branco basta, os
arquivos grandes são criados pelo próprio terminal; `typestr "texto"` em `lib.sh` digita tecla a tecla
no layout US; rodam em ~2 a 4 minutos no TCG, `QEMU_MEM=256M tools/perf/run.sh <img> bios <saida> 300 <cen>`):

| Cenário | O que faz |
|---|---|
| `w15b-probe.sh` | fumaça: o terminal no boot, `echo`, `ls`, Tab (`cd Doc` completa), `pwd`, `free` |
| `w15b-term.sh` | histórico (↑), Tab em caminho, Ctrl+L, `sleep 20` (a janela mostra "executando" e um segundo terminal responde), Ctrl+C, `ping`, `nslookup` (DNS real do QEMU), `ifconfig`, `curl`, `seq 10000` com PageUp e Ctrl+Home, recursão de função, `yes` (1 MiB de saída) sem travar |
| `w15b-edit.sh` | digitar, Ctrl+S (Salvar como: digitar substitui o nome sugerido), Ctrl+F/Ctrl+H e Alt+A, Ctrl+Z, fechar com alterações (Cancelar, depois Descartar), reabrir (`edit`), Ctrl+O, Ctrl+Q, um arquivo de 1 MB com 165 mil linhas (Ctrl+End/Home, PageDown) e outro de 60 mil linhas de 16 caracteres |
| `w15b-soak.sh` | 50x: editor pelo dock + um caractere + fechar com Descartar, e um terminal novo (Ctrl+N) que roda `seq 50` numa thread de comandos e fecha com Ctrl+D; com `--features perf-trace`, `tools/perf/w8-heap.sh` (medido: +832 B em 50 rodadas, 375 amostras) |
| `w15b-ui.sh` | roda do mouse no terminal, Ctrl+Shift+C/Ctrl+V, maximizar (a grade acompanha), clique, duplo clique e arrastar no editor, abrir um texto pelo Arquivos e a pergunta ao fechar com alterações |

Persistência: depois do cenário, `fs3_inject <disco.img> --ls /` lista o que ficou, e um novo
boot com o mesmo disco monta com `fsck clean`. Queda de energia no meio de uma cópia grande
(`kill -9` do QEMU): o próximo boot monta limpo; o arquivo copiado pela metade fica no disco
(menor que o original) e nada mais é afetado.

## 3. Boot em QEMU

Sem tela e sem KVM (TCG), BIOS e UEFI:

```bash
tools/qemu-headless.sh bios /tmp/kit 25      # serial.log + screen.png
QEMU_MEM=256M tools/qemu-headless.sh uefi /tmp/kit-uefi 40
KEEP_FS=1 tools/qemu-headless.sh bios /tmp/kit 25   # reaproveita fs.img (persistência, disco corrompido)
FS_SIZE=64K tools/qemu-headless.sh bios /tmp/kit 25 # disco antigo de 64 KiB (fica no OJFS v2)
```

O disco do filesystem do runner (`<outdir>/fs.img`) é um arquivo esparso de **64 MiB** por
padrão (o OJFS v3 exige >= 1 MiB; `tools/run.sh` e `tools/perf/run.sh` seguem a mesma
regra e aceitam `FS_SIZE`). Em um disco novo em branco o boot formata o v3 e semeia
`leiame.txt`, `notas.txt` e `Documentos/projeto.txt`; a serial mostra as linhas
`storage: ...` (montagem, migração, `TooSmall`, `Unknown`). Com `FS_SIZE=64K` o boot loga
`storage: disk too small for OJFS v3 (64 KiB), staying on v2` e o desktop funciona como antes.

**Migração v2 -> v3 no QEMU:** `cp <fs.img v2 de 64 KiB> disco.img && truncate -s 64M disco.img`
e `KEEP_FS=1` com esse arquivo copiado para `<outdir>/fs.img`: o primeiro boot loga
`storage: migrated OJFS v2 -> v3: ...`, o segundo só `OJFS v3 mounted` (sem remigrar), e
`cmp -n 65536` entre antes e depois prova que os 64 KiB iniciais não mudaram.

`tools/verify-boot.sh <outdir> [baseline]` constrói, sobe os dois modos e falha se
o boot não completar, se aparecer `KERNEL PANIC`/`FATAL` na serial, ou se o desktop
diferir da baseline (o HUD e o relógio são mascarados porque mudam a cada execução).
Para tirar uma baseline, rode o script uma vez num commit bom e passe o `outdir`
como segundo argumento nos seguintes.
Uma baseline tirada com o disco antigo de 64 KiB continua valendo com o disco de 64 MiB:
montar o v3 não muda um pixel do desktop (provado: 0 pixels em BIOS e UEFI).

### Cursor sem rastro (W20)

O cursor é desenhado só no framebuffer; um sprite que não é apagado deixa um rastro de
setas. O modelo puro (`kitsune_core::cursor`, testes com um framebuffer simulado) prova a
regra apagar-no-início / pintar-no-fim; o QEMU prova o kernel:

```bash
QEMU_MEM=256M tools/perf/run.sh <img> bios /tmp/w20c 400 tools/perf/scen/w20-cursor.sh
tools/perf/w20-cursor-check.sh /tmp/w20c        # "differing_pixels=0" em cada rodada
STEP=3 GAP=0.02 ...                              # devagar; STEP=250 GAP=0 = pacotes em rajada
QEMU_EXTRA="-device usb-ehci -device usb-tablet" ...   # o guest só tem PS/2: o tablet não muda nada
```

O cenário varre bordas de janela, a barra de título, a barra de tarefas e os cantos da tela com
`mouse_move` em rajadas (muitos pacotes PS/2 entre dois quadros), estaciona o cursor, tira
`rest<N>.png` e força um repaint completo (Alt+Tab e volta) para tirar `clean<N>.png` com
o cursor no mesmo lugar. Os dois têm de ser idênticos fora do HUD e do relógio (agora no centro do painel; as duas máscaras estão no script). Antes da
correção: 181 pixels de diferença por rodada (setas fantasma); depois: 0.

### Páginas compactadas (W20)

`tools/nettest-pages.py` serve páginas gzip/deflate difíceis (maiores que o limite de
resposta, `chunked`, deflate cru, CRC errado, cortada pelo servidor, `br`...);
`tools/perf/scen/w20-browser.sh` abre cada uma no navegador (cabeçalho dos dois arquivos
tem o comando, com a faixa SLIRP "pública" do `w18-net.sh`) e tira uma foto no topo e no
fim da página. Esperado: a página aparece (nunca "Falha ao descompactar") e, se for parcial,
uma faixa amarela diz por quê; "END OF PAGE" aparece só nas páginas decodificadas inteiras.

### Rede em QEMU

`tools/qemu-headless.sh` captura o tráfego da NIC em `<outdir>/net.pcap` e aceita estas
variáveis (documentadas no cabeçalho do script; `QEMU_RNG=none` tira o `-device virtio-rng-pci`
que os scripts passam por padrão):

```bash
QEMU_NIC=virtio tools/qemu-headless.sh bios /tmp/kit 25     # virtio-net em vez do NE2000 (ne2k, padrão; none = sem NIC)
QEMU_NIC=virtio QEMU_NETDEV="user,id=n0,net=192.168.77.0/24,host=192.168.77.2,dhcpstart=192.168.77.15,dns=192.168.77.3" \
  tools/qemu-headless.sh bios /tmp/kit 25                   # outra sub-rede; o convidado vê o host em 192.168.77.2
python3 -I tools/pcapsum.py /tmp/kit/net.pcap               # resumo: ARP, DHCP (tipo, unicast/broadcast), DNS, TCP, ICMP
```

O que cada prova mostra no resumo do pcap e na serial (todas feitas [M] com o commit que as
introduziu; a serial tem `net:` e `dns:`):

| Prova | Como | O que aparece |
|---|---|---|
| virtio-net, SLIRP padrão | `QEMU_NIC=virtio` | `net: using virtio-net`, `DHCP lease 10.0.2.15/24 ...`; pcap: DISCOVER, OFFER, REQUEST, ACK, ARP gratuito |
| página por virtio-net | `python3 -m http.server 8077 --bind 127.0.0.1` no host + navegador em `http://192.168.77.2:8077/` | `fetch: 279 bytes (status 200)` e a página na tela |
| NE2000 não regride | `tools/verify-boot.sh` (0 pixels diferentes) e a mesma página com `QEMU_NIC` padrão | idem |
| sem NIC | `QEMU_NIC=none` | `net: no network interface found`; o navegador mostra a falha na hora |
| entropia: com virtio-rng (padrão) | `tools/qemu-headless.sh bios /tmp/kit 22` | `virtio-rng @ pci ...`, `RNG: strong (256 bits from hardware ...)` |
| entropia: só jitter de temporização | `QEMU_RNG=none tools/qemu-headless.sh bios /tmp/kit 25 -- -cpu qemu64` (sem RDRAND/RDSEED; `-cpu max` os liga) | `RNG: no hardware generator; collecting timing jitter`, depois `RNG: pool seeded from timing jitter (N bits credited)` (N >= 128, em ~1-2 s) |
| *client random* do TLS | `openssl s_server -accept 4443 ...` no host + `tools/qemu-browse.sh <out> https://10.0.2.2:4443/` + `python3 -I tools/tls-hello.py <out>/net.pcap` | um *client random* por conexão, todos diferentes entre conexões e entre boots (`design/entropy.md` §5) |
| RENEW / REBIND / expiração | gancho **temporário** que força `lease_secs = Some(20)` e, nas variantes, descarta as respostas durante RENEWING (e REBINDING) | pcap: REQUEST unicast `10.0.2.15 -> 10.0.2.2` em T1 (~10 s); com a resposta descartada, REQUEST broadcast em T2 (~17,5 s); descartando também essa, na expiração (20 s) DISCOVER e novo lease |
| ping | gancho **temporário** no boot chamando `netd::ping_us` | `ping gateway (10.0.2.2): reply rtt 525 us = 1 ms`; pcap: ICMP tipo 8 e 0; alvo sem ARP: `host did not answer ARP`; o próprio IP: `invalid target address` |
| failover do DNS | gancho temporário com DNS `10.0.2.250` (morto) e `10.0.2.3`, navegador em `http://example.com/` | `dns: no answer for example.com, trying 10.0.2.3`, `dns: example.com -> ... (2 attempts)`; pcap: ARP sem resposta para `.250`, depois a consulta para `.3` |

Os ganchos são do padrão acima (remova antes de commitar); o lease de 20 s existe só para
a prova (o mínimo de retransmissão do RFC é 60 s, mas é limitado pelo fim da etapa).

**Autoteste de armazenamento** (build `perf-trace` com `KITSUNE_STORAGE_SELFTEST=1`):

```bash
KITSUNE_STORAGE_SELFTEST=1 cargo build --release -p os --features perf-trace
```

depois do mount o boot cria `/selftest`, escreve 2 MiB com um padrão, relê e compara,
roda `fsck`, apaga e loga na serial `storage: selftest ...` com os tempos e MiB/s
(em TCG, PIO de verdade). É a única forma de exercitar o v3 no kernel enquanto o desktop
ainda usa o v2. Medido num host compartilhado (varia 30%): escrita de 2 MiB em 0,4 a 0,6 s
(3,4 a 5,3 MiB/s), leitura de 2 MiB de 0,4 a 0,6 s (3,2 a 5,3 MiB/s), `fsck` de um volume
pequeno abaixo de 5 ms, 13 a 23 ms por arquivo pequeno (4 barreiras de flush cada).
O gargalo é o PIO emulado (um setor por trânsito de porta), não o ceder a CPU entre
setores: sem o `yield` deu o mesmo.

### Provando falhas (padrão usado na auditoria)

Para provar que uma falha é reportada, use um gancho **temporário** de build, rode,
e remova antes de commitar:

```rust
match option_env!("OSJ_FAULT") {
    Some("ud2")   => unsafe { core::arch::asm!("ud2") },
    Some("pf")    => unsafe { core::ptr::read_volatile(0xdead_0000usize as *const u8); },
    Some("panic") => panic!("test panic"),
    Some("oom")   => { let v = alloc::vec![0u8; 200 << 20]; core::hint::black_box(&v); }
    _ => {}
}
```

```bash
OSJ_FAULT=ud2 cargo build --release -p os && tools/qemu-headless.sh bios /tmp/f 14
grep -E "FATAL|PANIC" /tmp/f/serial.log
```

Para confirmar que **não** houve triple fault, passe `-- -d cpu_reset -D /tmp/f.qlog`
ao script e procure `Triple fault` no log (duas linhas `CPU Reset` são o reset
normal de ligar a máquina).

## 4. Desempenho

QEMU sem aceleração **não** representa hardware real. Compare proporções.

| Ferramenta | Uso |
|---|---|
| HUD do desktop (Ctrl+Alt+H; canto superior esquerdo) | ms/frame, fps, draws/s, heap, threads |
| `cargo build --release -p os --features perf-trace` | estatísticas por segundo na serial (`[trace]`): custo por etapa de render, ISR, alocações, latência de entrada |
| `tools/perf/run.sh`, `ab.sh`, `cmp.sh` | cenários scriptados (mouse/teclas pelo monitor do QEMU), A/B intercalado, `-icount` para razões estáveis |
| `tools/perf/scen/w8-*.sh`, `w8-heap.sh` | window manager: várias instâncias (`w8-multi`), maximizar/minimizar/Alt+Tab/redimensionar (`w8-wm`), 30+ janelas (`w8-stress`), soak de abrir/fechar 100x com a ocupação exata do heap (`w8-soak` + `w8-heap.sh`, build `perf-trace`) |
| `tools/perf/scen/w13-*.sh` | plataforma de apps: quatro apps ao mesmo tempo (`w13-apps`), app hostil (`w13-hostile`, só com o gancho temporário descrito abaixo), instalar/remover pelo Files (`w13-install`), CPU por app no app Tarefas (`w13-cpu`), soak de abrir/fechar apps 100x (`w13-soak` + `w8-heap.sh`, build `perf-trace`) |
| `cd bench && cargo bench` | microbenchmarks no host (criterion), crate fora do workspace |

Os marcos de boot (`[trace] boot + N ms`) saem na serial em qualquer build.

### Interface (W22): testes e custo de quadro

Testes novos no `kitsune_core` (todos no host; o kernel só liga o framebuffer a eles):

| Módulo | Testes | O que prova |
|---|---|---|
| `ttf`, `glyph`, `fontcache`, `textlayout` | 8, 9, 7, 6 | leitor TrueType total (fontes hostis não estouram nem entram em laço), contornos compostos, cobertura exata do rasterizador, cache por (peso, tamanho), medição, quebra e reticências, célula da fonte monoespaçada |
| `raster` | 14 | pré-multiplicação e *source-over* sem deriva, máscaras de canto, retângulos e contornos AA, perfis de sombra simétricos, desfoque, reamostragem |
| `anim` | 16 | bezier monotônico, mola sem divergir, `Tween` que reaponta sem salto, animação de janela interrompível, *reduzir movimento*, salto de lançamento |
| `style`, `chrome`, `widgets` | 4, 11, 6 | paletas e aparência automática pela hora, geometria e acerto do painel superior (relógio centralizado, pílula, pontos das áreas de trabalho), menus, trilho do Apps, Busca, popovers (Configurações rápidas, calendário com notificações) e banners; segmentado, switch, controle deslizante, barra de rolagem |
| `snap`, `taskbar`, `launcher` | 8, 7, 6 | zonas de encaixe nas bordas e cantos, divisão exata da área útil, tamanho mínimo, `Alt+setas`; barra de tarefas (layout, hit test, indicador, clique, reordenar com vizinhos que abrem espaço); categorias, filtro, recentes |
| `brand` | 14 | a marca da raposa: polígonos dentro da grade e sem degenerados, simetria, mono de uma cor, cobertura e tinta distinta de olhos e nariz em 16 a 128 px, coroa de caudas, SVG, hash dourado do tile de 32 px (atualização: veja o comentário de `GOLDEN_TILE_32` em `kitsune_core/src/ui/brand/tests.rs`), tela de contato com `BRAND_SHEET=/tmp/b.ppm cargo test -p kitsune_core dump_brand -- --ignored` |
| `iconart`, `cursor` | 7, 4 | todos os ícones e glifos têm conteúdo, cantos transparentes, determinismo; sprites dentro da caixa, ponto quente sobre a forma |
| `search` | 5 | ranqueamento, dobra de acentos, calculadora exata (overflow recusado, nunca embrulhado), nenhum texto curto derruba o *parser* |
| `notify`, `settings`, `wallpaper`, `window`, `layout`, `winman` | 10, 15, 10, 21, 40, 44 | banners deslizando, chaves `appearance`/`reduce_motion` totais, esquemas claro e escuro, botões à direita (`title_layout`), encaixe e soltar pela barra, áreas de trabalho (`switch_workspace`, `move_to_workspace`), área útil entre o painel e a barra de tarefas |

Cenários de tela (cada um fotografa e a imagem é revisada em claro e escuro). `tools/perf/scen/w33-brand.sh` (UEFI e BIOS, claro e escuro com `QEMU_EXTRA="-rtc base=2026-10-08T12:00:00"`) fotografa a vinheta de abertura, o painel e a barra de tarefas, o Apps, o menu do sistema, Ajustes > Sobre e `kitsune://sobre`:
`w22-look`, `w22-apps` (todos os apps nas duas aparências), `w22-shell` (Busca, folha, galeria,
HUD, menus de contexto), `w22-polish` (menus, Controles, calendário, Apps, Busca, folha),
`w22-wm` (Alt+Tab, muitas janelas), `w22-toast` (banners; precisa de um gancho temporário de build, como o `w14-toast`), `w22-anim` (abrir, zoom, minimizar, restaurar em voo),
`w22-bar` (botão de menu da janela, Configurações rápidas), `w22-splash`, `w22-readme` (capturas do README, em UEFI).
A identidade própria (W26) tem cenários novos: `w26-wm` (botões à direita, encaixe com pré-visualização,
`Alt+setas`, menu da janela), `w26-panel` (painel, calendário com notificações, Configurações rápidas),
`w26-taskbar` (indicadores, minimizar, reordenar, menu, mostrar área de trabalho), `w26-look`
(os seis papéis de parede, ícones, ponteiro, aba *Shell* da galeria), `w26-launcher` (trilho,
recentes) e `w26-workspaces`. Em claro: `QEMU_EXTRA="-rtc base=2026-10-08T12:00:00"`.
`tools/perf/lib.sh` tem `dock_icon <nome>` (posição de cada ícone da barra de tarefas), `panel_item`,
`quick_tile` e `light_mode` e `move` agora
divide saltos grandes em passos de 100 px (um pacote PS/2 grande estoura).

Custo de quadro (QEMU/TCG sem KVM, 1280x720, `perf-trace`, `tools/perf/summ.py`; média por caminho;
antes = árvore no início do trabalho, mesmos cenários):

| Cenário | antes | depois |
|---|---|---|
| ocioso (CPU por segundo) | 0 + um tique de relógio de 0,33 ms | 0 + um tique de 0,35 ms |
| arrastar a janela (quadro de dano) | 2,32 ms | 3,67 ms |
| abrir e fechar uma janela (quadro de animação) | 11,3 ms | 4,1 ms |
| Apps: quadro de hover | 5,05 ms | 0,64 ms |
| Apps: quadro que abre | 14,0 ms | 34,3 ms |
| barra de apps varrida pelo ponteiro (ampliação) | n/a | 2,98 ms por quadro |
| texto, 48 caracteres | 283 672 ciclos (fonte 8x8) | 39 550 ciclos (13 px, vetorial) |
| retângulo arredondado 512x320, cheio | 874 120 ciclos | 1 113 396 ciclos (AA) |
| atlas de glifos no boot | n/a | 917 glifos, 61 KiB, 34 ms |

O arrasto custa mais porque a janela agora tem cantos e sombra com anti-aliasing e um título de
texto de verdade; fica abaixo da meta de 4 ms. Abrir o Apps paga uma vez a captura do fundo
borrado (um quarto da resolução). O ocioso continua em zero quadros fora do tique do relógio.

### Apps do sistema (W25): Tarefas, Registro, Ajustes, Calculadora, banners

A contagem do `kitsune_core` passou de 2497 para 2532 testes (todos no host). A lógica nova mora no
core; o kernel só desenha e liga a entrada:

| Módulo | Testes | O que prova |
|---|---|---|
| `activity` (novo) | 16 | nomes amigáveis e de sistema, formatação pt-BR, suavização que nunca ultrapassa o alvo, deslizar e parar, amostras na ordem, ponteiro para amostra, contadores que voltam, taxa por segundo, média de carga, níveis de pressão da memória, tabela ordenável por toda coluna nos dois sentidos, estável e sem acento, busca e totais |
| `calc` | 26 (eram 18) | percentual (de um operando para + e -, sozinho e com ×), troca de sinal, memória (guardar, somar, subtrair, recuperar, limpar), as quatro últimas operações, expressão pendente, vírgula decimal e negativos minúsculos sem sinal, `pretty` com separador de milhar |
| `layout` | 17 (eram 16) | as regiões da calculadora empilham dentro da janela, teclado 6x4 e acerto por tecla |
| `settings` | 19 (eram 15) | tempo do banner e zoom da barra dentro da faixa, campos novos ida e volta, a cidade concorda com o deslocamento, tabela de 52 cidades ordenada, válida e encontrável |
| `notify`, `klog`, `chrome` | 12, 28, 13 | vida própria de cada banner (limitada) e a linha que se esgota; visão do registro a partir de qualquer linha e `FixedBuf` com UTF-8 (Latin-1 como reserva); o zoom da barra escala o relevo |

Cenários (cada um fotografa, a imagem é revisada em claro e escuro): `w25-tarefas`, `w25-tarefas2`,
`w25-tarefas3` (abas, passar o ponteiro sobre o gráfico, tabela com seleção e folha de confirmação,
32 processos com a roda, troca rápida de abas, janela no tamanho mínimo), `w25-log`, `w25-calc`,
`w25-settings` (nove seções, tema, destaque, papel de parede, zoom, teclado, busca de fuso),
`w25-shots` (capturas do README, em UEFI; o banner precisa de um gancho temporário de build),
`w25-perf` e `w25-perf-hidden` (custo de uma janela Tarefas visível e minimizada).
O registro foi ainda testado com um gancho temporário que enche o anel de 771 linhas.

Custo de quadro (QEMU/TCG sem KVM, 1280x720, `perf-trace`; base = `bc31f5e`, mesmos cenários e
mesma máquina, execuções intercaladas; o hospedeiro é compartilhado, então repetições do mesmo
cenário variam uns 8 %):

| Cenário | base | depois |
|---|---|---|
| ocioso (um tique de relógio por segundo) | 0,30 a 0,33 ms | 0,31 a 0,36 ms |
| arrastar a janela, quadro de dano (4 execuções cada) | 6,0 a 6,8 ms (média 6,3) | 6,5 a 7,5 ms (média 6,9) |
| Tarefas visível na aba CPU, quadro do gráfico em movimento | n/a | média 0,77 ms (pior 8,5 ms), 96 quadros por segundo contando os pulados |
| Tarefas visível, o primeiro e o último quadro de cada amostra de 1 s | n/a | 21 ms (reconstrução) e 15 ms (assentar), uma vez por segundo |
| Tarefas visível, CPU ocupada | n/a | 8,2 % |
| Tarefas minimizada | n/a | 0 além do tique de relógio (0,31 ms), CPU 0,07 % |

A meta de 4 ms para o quadro do gráfico fica folgada (0,77 ms): enquanto o gráfico desliza, o
compositor repinta só o título e o gráfico, um quadro a cada 20 ms. O arrasto ficou perto de 10 %
mais caro: o tempo dos retângulos arredondados no rastro por primitiva dobra mesmo com o mesmo
número de chamadas (a janela arrastada é o Terminal, que não mudou), e a soma da composição só sobe
uns 5 %; não achei a causa e não é efeito do repintar parcial (desligado numa imagem de teste, o
resultado é o mesmo). Fica como lacuna conhecida.
### Arquivos, Imagens, Editor e Terminal (W23): testes e custo de quadro

Testes novos no `kitsune_core` (todos no host; o kernel só desenha e roteia; 2497 → 2590 passando, 3 ignorados):

| Módulo | Testes | O que prova |
|---|---|---|
| `appart` | 8 (+1 ignorado, que gera a folha de contato) | cada tipo de arquivo tem conteúdo em todos os tamanhos, cantos transparentes, ícones distintos e determinísticos, o destaque tinge pastas e apps e não documentos, glifos na cor pedida, pares espelhados, tamanhos limitados |
| `fileman::ui` | 32 | layout da janela (barra estreita com busca aberta recolhe o caminho), colunas, migalhas, acerto, geometria dos itens (lista e ícones), itens visíveis sem fora-por-um, seleção por retângulo, rolagem de borda, plano de soltar (mover/copiar, pasta dentro de si mesma recusada, Lixeira), `Scroller` (mola criticamente amortecida), chaves de busca com acentos, tipo de pré-visualização e texto |
| `fileman` (estado) | 93 no módulo | filtro da pasta, lugares, `TextInput` com seleção e bytes Latin-1, ordenação, seleção por nome |
| `viewer::ui` | 22 | layout (com e sem faixa), acerto, faixa de miniaturas (rolagem, visíveis, ordem de criação), `Inertia`, `Slideshow`, `RotMap` e limites girados |
| `editor2::ui` | 15 | grade e margem, célula sob o mouse com cliques na margem e fora, trechos de seleção, guias de indentação, folhas (tamanho, lista, lugares, botões), barra de buscar (controles não se sobrepõem, acerto de cada um) |
| `editor2` (busca, diálogo) | +3 | foco do campo da barra, nome inicial "fresco", barra de estado |
| `termui` | 6 | grade dentro da janela, mouse → célula com limites, ordem e trechos da seleção, palavras, extração de texto sem pânico, prompt separado |
| `anim` | +2 | piscar suave do cursor (monótono, sólido depois da entrada e depois de 12 s) |
| `settings` | +2 | `editor_font`/`terminal_font` totais com limites, degraus de `font_step` |

Cenários de tela (cada um fotografa; todas as imagens são revisadas em claro e escuro,
`QEMU_MEM=256M tools/perf/run.sh <img> bios <saida> 300 <cen>`; `FS_IMG` é um disco preparado com o
`fs3_inject`: `/Imagens/*.png` (inclusive `transparente.png` e `corrompida.png`), `/Documentos`,
`/Projetos`, `/big.bin` de 3 MB, `/leiame.txt`, `--files 2000 /many` e
`/etc/kitsune.conf` com `appearance=light` ou `appearance=dark`):

| Cenário | O que faz |
|---|---|
| `w23-files.sh` | lista, renomear, menu de contexto, ordenar, Imagens, ícones, pré-visualização (Espaço), busca, seleção por retângulo, arrastar e soltar com destino destacado, confirmação, propriedades, Lixeira vazia |
| `w23-many.sh` | `/many` (2000 arquivos): roda, PageDown, setas, Home/End, arrastar a barra de rolagem |
| `w23-viewer.sh` | ajustar, painel de informações, preencher, zoom, giro (quadro no meio), PNG transparente, imagem corrompida, apresentação, folha de salvar |
| `w23-editor.sh` | digitar, seleção, buscar, substituir, Abrir, Ctrl +/-/0, Salvar como, salvo, a pergunta ao fechar, abrir pelo diálogo |
| `w23-terminal.sh` | prompt, seleção com o mouse, copiar e colar, rolagem, Ctrl +/-/0, "executando" |
| `w23-idle.sh` | desktop ocioso com Imagens, Arquivos, Terminal e Editor abertos; `tools/perf/idle_tail.py <saida>` conta os quadros por segundo dos últimos segundos |

Custo de quadro (QEMU/TCG sem KVM, 1280x720, `perf-trace`, `tools/perf/summ.py`; a máquina é
compartilhada, então cada cenário rodou duas vezes; antes = `bc31f5e`, mesmo disco e mesmos cenários):

| Cenário | antes | depois |
|---|---|---|
| ocioso, quadros por segundo depois do boot | 1 (relógio) | 1 (relógio) |
| ocioso com Imagens, Arquivos, Terminal e Editor abertos (`w23-idle.sh`) | n/a | 1 (relógio), nenhum quadro de app |
| arrastar a janela (quadro de dano, média das duas rodadas) | 6,4 e 7,2 ms | 8,0 e 6,7 ms |
| pasta de 2000 arquivos, rolagem (quadro de animação / estável) | 16,4 ms (estável, pico 40 ms) | 9,3 ms (animação, pico 49 ms) |

O cursor que pisca pede quadros só na janela em foco, por 12 s depois da última tecla ou clique; uma
janela que nunca recebeu entrada não pisca e não pede quadros (corrigido durante o trabalho: o
terminal aberto no boot fazia cerca de cem quadros por segundo nos primeiros 12 s).
### Navegador na W24: testes, fuzz e custo

Testes novos no `kitsune_core` (todos no host): `web::layout_tests` (quebra por largura medida,
zoom 50 a 300, tabelas, estilos aninhados, palavras longas, texto vazio, páginas enormes,
alinhamento vertical, colunas com largura em px), `web::css` (índice de regras), `browser::tabs`,
`browser::{cert,errors,motion,pages}`, `browser::ui_tests` (abas compartilham favoritos,
recarregar, parar, recentes), `layout` (geometria da moldura, balão, busca, erro, nova aba).
Total: 2631 no `kitsune_core` (eram 2497).

Fuzz: `web_parse` (agora também tabelas, seletores, vários zooms e métricas hostis),
`html_img_form` (formas de tabela, estilo, listas, abas, páginas `kitsune://`) e `x509_parse`
(resumo de certificado para o balão). Execuções: `web_parse` 361 s (1178 execuções, entradas pesadas), `html_img_form` 331 s (10 798) e `x509_parse` 121 s (1,46 milhão), sem falha restante; `html_img_form` achou um marcador de lista fora da página (corrigido, `fuzz/regressions/html_img_form/w24-list-marker-off-page`).

Custo (QEMU `-icount shift=0`, determinístico, 1 GHz virtual; mesmo cenário e mesma página de
2000 nós; antes = base `bc31f5e` com os mesmos ganchos `[trace] browser ...`):

| Medida | antes | depois |
|---|---|---|
| análise (46 KB, `/big?n=2000`) | 8,9 ms | 20,6 ms (35 ms antes das otimizações) |
| layout da página de 2000 nós | 15,5 ms (5679 comandos, fonte fixa) | 25,4 ms (2019 comandos; 62,7 ms antes das otimizações) |
| página pequena (`/type`): carga até o primeiro quadro | 10 ms | 19 ms |
| página grande: carga até o primeiro quadro | 33,5 ms | 61 ms (116 ms antes das otimizações) |
| quadro de rolagem (roda, setas) | 14 a 21 ms (recompõe a área de trabalho) | 5,2 a 6,3 ms (só a área do cliente) |
| salto de página (PgDn) numa página grande | 14 a 21 ms | 11 ms |
| navegador aberto e ocioso | 0 quadros | 0 quadros (só o tique do relógio) |

O texto proporcional custa mais que a fonte fixa na carga (cada palavra é medida na fonte
real); o cache de glifos e kerning, o índice de regras e menos alocações cortaram o layout
pela metade. Cenários: `w24-pages.sh`, `w24-chrome.sh`, `w24-chrome2.sh`, `w24-errors.sh`,
`w24-internal.sh`, `w24-overlap.sh`, `w24-perf.sh` (+ `tools/perf/w24-perf.py`), `w24-readme.sh`;
servidores: `tools/w24-site.py` (:8079) e `tools/nettest-server.py` (HTTPS autoassinado :8078).
Os números de tempo real do TCG variam com a carga do hospedeiro; use `-icount` para comparar.

### Bloqueador de anúncios (W53)

Testes no host: `cargo test -p kitsune_core shields` (listas, casamento, contagem, lista de sites, imagens e folhas
bloqueadas, cosmético). No QEMU, com a página `/ads` do `tools/w24-site.py` (`2>> /tmp/w53-requests.log` guarda o que o
guest pediu): `w53-shields.sh` (`REQLOG=/tmp/w53-requests.log`; escreve `w53-result.txt` com PASS/FAIL e as capturas),
`w53-persist-1.sh` + `w53-persist-2.sh` (a escolha por site depois de reiniciar, com `FS_IMG`) e `w53-settings.sh` (a chave
global). As posições do escudo e da chave são as da janela padrão em BIOS 1280x720 (`SHIELD_X`, `SWITCH_X`... mudam por
variável de ambiente). Projeto e limites: [`design/tls-browser.md`](design/tls-browser.md) §11.

### Som (W56)

Testes no host: `cargo test -p kitsune_core hw::audio` (fila, limites, reamostragem de 8 a 96 kHz conferida por
cruzamentos de zero, saturação, volumes, mudo, relógio "tocado" contra o dispositivo, drenar), `hw::ac97` (descritores e
posição a partir de CIV/PICB/SR, parada e retomada), `format::wav` (8/16/24/32 bits, float, EXTENSIBLE, truncado, lixo),
`settings` (volume e mudo), `shell::tests::process` (os comandos `play`, `tone`, `beep`, `volume`) e
`cargo test -p kitsune-userabi` (`AudioStatus`). No QEMU, o próprio QEMU grava o que o guest toca:

```bash
OUT=/tmp/w56; QEMU_MEM=256M \
QEMU_EXTRA="-audiodev wav,id=snd0,path=$OUT-audio.wav,out.frequency=48000 -device AC97,audiodev=snd0" \
bash tools/perf/run.sh IMG bios $OUT 600 tools/perf/scen/w56-audio.sh     # veredito em $OUT/verdict.txt
QEMU_MEM=256M bash tools/perf/run.sh IMG bios /tmp/w56n 200 tools/perf/scen/w56-audio-nodev.sh
```

`tools/perf/audio-check.py CAPTURA.wav PLANO [--serial serial.log]` mede cada som da gravação (frequência por FFT, duração,
nível relativo, pureza, silêncio exato depois, canais iguais) e o relógio impresso por `play --clock`; sem `numpy` mede a
frequência por cruzamentos de zero. Não cobre: o backend `dsound` do Windows e hardware real. Projeto:
[`design/audio.md`](design/audio.md).

**CPU do navegador (W54).** `tools/perf/scen/w54-browser-idle.sh` abre uma página de `tools/w24-site.py` (`PAGE=modern`,
`bigcss` ou `js`), deixa-a parada e depois rola; com um build `--features perf-trace`, `tools/perf/idle_tail.py <saída> 10`
mostra os quadros por segundo e a linha `cpu samples` de cada segundo do serial deve ficar em `idle=250` com a página
parada (nenhuma thread ocupada). Para sites reais: `w50-real-site.sh` com `SITES`, `CONTINUE=1` e a mesma leitura.
Projeto: [`design/tls-browser.md`](design/tls-browser.md) §11b.

### Mídia: WebM, VP9 e Opus (W57)

Projeto em [`design/midia.md`](design/midia.md). Três camadas:

1. **Demuxer** (`kitsune_core::webm`): `cargo test -p kitsune_core webm` (pacotes idênticos ao `ffprobe` do arquivo
   `testdata/small.webm`, qualquer fatiamento, busca por cue, tamanhos desconhecidos, entradas hostis) e o alvo de
   fuzz `webm_demux`.
2. **Decodificadores no hospedeiro**: `tools/media/gen-test-media.sh M` gera, com o `ffmpeg` do hospedeiro, os
   arquivos (320x180, 640x360 e 1280x720, VP9 + Opus, 3 s) e as referências (`framemd5` do libvpx, PCM do ffmpeg, RGBA
   do primeiro quadro); `tools/media/check-host.sh M` compila o **mesmo C e o mesmo pipeline** do `mediadec` com o
   clang do hospedeiro (`tools/media/mediacheck`) e confere: MD5 de cada quadro idêntico, PCM com a mesma contagem de
   amostras e SNR > 40 dB, cor a no máximo 1 nível da fórmula exata, o servidor de mensagens em memória
   (`--serve-test`) e um arquivo corrompido sem queda. Deixa em `M/*.pcm.md5` o MD5 do PCM do hospedeiro.
3. **No QEMU**: `MEDIA=M QEMU_MEM=256M bash tools/perf/run.sh IMG bios OUT 1500 tools/perf/scen/w57-mediadec.sh`. O
   cenário serve `M` por HTTP (porta 8857), baixa os arquivos com `wget` no Terminal, roda `run mediadec X.webm --md5` e
   compara cada MD5 de quadro com o `framemd5` e o MD5 do áudio com o do hospedeiro (o Opus em ponto fixo é inteiro: os
   bytes são os mesmos), depois `--serve-test`, um arquivo cortado e um corrompido (terminam sozinhos, o sistema segue),
   `--info`, `--rgb 1280x720` e `--show` (captura `d-show.png`). Veredito em `OUT/verdict.txt`, com os fps de cada
   tamanho.
### DOM para frameworks (W59): Preact, Vue e Workers

Host: `cargo test -p kitsune_core script:: same_document html_tests` (mensagens de entrada, retrato do layout,
`WebStorage`, workers, histórico do mesmo documento, nomes de atributo) e `node tools/js-dom-check.js
user-apps/jsrun/src/bin/jsrun tools/site-assets`: carrega os arquivos do prelúdio na ordem do `jsrun.rs` num `vm` do
Node (sem `Intl`, como o QuickJS), faz o papel do navegador (o `init` montado de cada página, teclas com o texto do
campo, cliques, foco, retratos de layout, `store`, `pop`, e um segundo contexto para cada worker) e confere 34 APIs,
o TodoMVC de Preact + htm e o app Vue 3 de `tools/site-assets`, e os workers. Com
`KITSUNE_INIT=<binário do exemplo script_init>` (`cargo build -p kitsune_core --example script_init`) a árvore vem do
parser HTML do navegador em vez do parser de fragmentos do `dom.js`. QEMU:

```bash
python3 tools/w24-site.py 8279 &        # porta própria (também usa 8278); /preact, /vue, /worker, /assets/...
PORT=8279 QEMU_MEM=256M QEMU_NETDEV="user,id=n0,net=203.0.113.0/24,host=203.0.113.5,dhcpstart=203.0.113.15,dns=203.0.113.3" \
  tools/perf/run.sh <img> bios /tmp/w59 1000 tools/perf/scen/w59-spa.sh     # ONLY=preact|vue|worker roda uma parte
```

O cenário digita (`typestr`, `key`) e clica (`goto` + `click`) pelo monitor: as páginas imprimem onde estão os seus
controles (`... at nome=x,y`, de `getBoundingClientRect` num `ResizeObserver`) e o navegador imprime onde a página
está na tela (`page at X,Y zoom Z`). Veredito em `$OUT/verdict.txt` (24 verificações: o estado que as páginas
imprimem depois de cada ação, o `pushState` e o `webstorage: saved` do log), passos em `steps.txt`, capturas `a`–`h`.
As páginas cabem na janela do navegador como ela abre (BIOS, 1280x720); o cenário não rola. Projeto:
[`design/tls-browser.md`](design/tls-browser.md) §14.

### Player: `<video>`, `<audio>` e MSE (W58)

Projeto em [`design/tls-browser.md`](design/tls-browser.md) §14. Hospedeiro:

```bash
cargo test -j2 -p kitsune_core media                          # TimeRanges, track buffer, relógio, player, mensagens, controles
cargo test -j2 -p kitsune_core layout_tests::media            # caixa de <video>/<audio>
node tools/js-media-check.js user-apps/jsrun/src/bin/jsrun    # a API em JavaScript com um navegador falso (52)
target/mediacheck/release/mediacheck V.webm --serve-test         # o decodificador do player (src inteiro, seek, fetch)
target/mediacheck/release/mediacheck V.webm --serve-mse A.webm   # MSE: vídeo e som em fontes separadas
```

(`tools/media/check-host.sh M` compila o `mediacheck` e roda o `--serve-test`.) No QEMU, com som gravado:

```bash
python3 tools/w24-site.py 8079 &        # gera os vídeos de teste com o ffmpeg em $W24_MEDIA (padrão /tmp/w24media)
OUT=/tmp/w58; QEMU_MEM=256M PORT=8079 \
QEMU_NETDEV="user,id=n0,net=203.0.113.0/24,host=203.0.113.5,dhcpstart=203.0.113.15,dns=203.0.113.3" \
QEMU_EXTRA="-audiodev wav,id=snd0,path=$OUT-audio.wav,out.frequency=48000 -device AC97,audiodev=snd0" \
bash tools/perf/run.sh IMG bios $OUT 900 tools/perf/scen/w58-video.sh   # veredito em $OUT/verdict.txt
```

O cenário abre `/video` (toca, fim, seek para 1 s, clique no play dos controles) e `/mse` (dois `SourceBuffer`s por
`Range`), confere os eventos que a página imprime (`media: ...`), compara capturas com 1 s de intervalo (a imagem e a
barra de tempo andam), lê as linhas `media: element N played ...` do serial (quadros mostrados/descartados, relógio) e
mede o som gravado com `tools/perf/audio-check.py` (cinco tons de 440 Hz com 3/2/3/3/2 s). Sem `QEMU_EXTRA` toca mudo
pelo relógio do sistema. `w58-real.sh` (`PAGES='real?src=URL ...'`) abre um WebM qualquer e registra se tocou e a
vazão; `/relay?u=URL` faz o servidor de teste baixar o arquivo uma vez e servi-lo com `Range`.

### Rede dos scripts (W55): fetch, XHR, cookies, CORS, keep-alive

Host: `cargo test -p kitsune_core http1 cookies cors script` (leitor HTTP/1.1 e chunked, Range, cookies, CORS, as
mensagens `F`/`X`/`C`) e `node tools/js-net-check.js user-apps/jsrun/src/bin/jsrun` (o `dom.js` + `net.js` num `vm` do
Node com um navegador falso: `fetch`, streams, abort, XHR, `FormData`, `TextDecoder`). QEMU:

```bash
python3 tools/w24-site.py 8079 --tls 8443 &      # HTTP/1.1 keep-alive em 8079, outra origem em 8078, HTTPS em 8443
QEMU_MEM=256M QEMU_NETDEV="user,id=n0,net=203.0.113.0/24,host=203.0.113.5,dhcpstart=203.0.113.15,dns=203.0.113.3" \
  PORT=8079 TLS_PORT=8443 tools/perf/run.sh <img> bios /tmp/w55 900 tools/perf/scen/w55-net-js.sh
```

O veredito fica em `$OUT/verdict.txt` (as 23 verificações que o script da página `/net` imprime como `net: PASS ...`
e 8 do log do sistema) e as vazões em `$OUT/speed.txt`. `NET_CHECKS=0 SPEED_HTTP=1` mede só a vazão (http e https);
`QEMU_NIC=virtio-net-pci` troca a NE2000 pela virtio. A parte https clica em "continuar mesmo assim" (certificado
autoassinado). Projeto: [`design/tls-browser.md`](design/tls-browser.md) §13.

### Compositor (W27): teste diferencial, oráculo no QEMU e custo de quadro

Projeto em [`design/compositor.md`](design/compositor.md). O desktop piscava com várias janelas (sombras
sumindo, o Snake piscando, uma janela pintada acima de outra que está acima dela); o compositor
tinha vários caminhos e uma assinatura de cache que precisava ser estendida a cada recurso novo.
Agora há um caminho só, e a corretude é **testada contra um redesenho completo**:

| Camada | Comando | O que prova |
|---|---|---|
| Host, `kitsune_core::compositor` (52 testes, ~40 s) | `cargo test -p kitsune_core compositor` | `Region` (disjunta, só cresce, subtração exata), o motor em cenas feitas à mão, e o **teste diferencial**: um simulador de área de trabalho (janelas, áreas de trabalho, z-order, animações com opacidade, janelas "vivas" e "jogo", painel, barra, popover, toast, pré-visualização de encaixe) com um pintor de modelo (cor única por janela, faixa de título, cantos recortados, anel de sombra translúcido em aritmética inteira, escritas fora do footprint são acusadas). 1500 históricos x 60 quadros e 40 x 400, de 1 a 3 eventos por quadro: depois de **cada** quadro o resultado incremental (só com o dano do motor) é idêntico, byte a byte, ao redesenho completo. A mensagem de falha traz o seed e as operações |
| Host, casos dirigidos | idem | sombra sob a vizinha, janela acima de uma animada, sombra cortada pela borda do dano, janela saindo da tela pelos quatro lados, maximizada, translúcida, dano de 1 pixel, áreas de trabalho, overlays, e os dois bugs reportados (Tarefas + Snake; Editor sobre Tarefas, com a sombra afirmada presente) |
| Host, **testes de mutação** | idem (`tests/mutants.rs`) | quebra-se de propósito cada regra e exige-se que o teste diferencial falhe: esquecer o footprint antigo, as trocas de z-order, a camada removida, o `look`, o `dirty`; omitir a sombra do footprint; declarar os cantos arredondados como opacos; esquecer o `dirty`, o `look`, a invalidação do relógio. 11 mutantes, todos pegos |
| Fuzz | `cd fuzz && cargo fuzz run compositor_ops -- -max_total_time=300 -print_final_stats=1` | o mesmo simulador dirigido pela entrada: incremental == completo, nenhuma escrita fora do footprint, quadro ocioso não planeja nada |
| QEMU, oráculo | `tools/perf/scen/w27-oracle.sh` + `tools/perf/w27-oracle-check.sh <out>` | muitas janelas (Terminal, Editor, Calculadora, Arquivos, Ajustes, Snake, Tarefas): mover, redimensionar, focar, minimizar, restaurar, encaixar, maximizar, áreas de trabalho, popovers, menus, Alt+Tab. Depois de cada passo, `rest<N>.png` (o que o compositor incremental deixou) contra `clean<N>.png` (o mesmo estado em modo referência, Ctrl+Alt+R: todo quadro recomposto do zero). Conjunto A (janelas estáticas): 0 pixels (até 200, o cursor de texto que pisca; `W27_TOL_CARET`). Conjunto C (`W27_SET=c`): o navegador com uma página longa (`kitsune://sobre`) rolada por teclas ao lado de um Editor, de um Terminal à frente e de Arquivos (faixa de seleção) e Ajustes (controle deslizante), mais uma rajada. Conjunto B (`W27_SET=b`: Snake, um terminal executando `sleep`, Tarefas): duas fotos de verdade a mais dizem quais pixels mudam sozinhos e o verificador os ignora, e tolera 6000 pixels dos números das Tarefas (`W27_TOL`) |
| QEMU, piscadas | `tools/perf/w27-burst-check.sh <out> b2_ 0` | rajada de 12 fotos a cada 0,2 s: sem Tarefas, quadros consecutivos idênticos (0 pixels); com Tarefas visíveis, só os números dela mudam (até ~3800 px por passo) |
| QEMU, modo verify | `W27_VERIFY=1 ...` / Ctrl+Alt+V | cada quadro é comparado com o redesenho completo (que não confia em footprint nenhum); divergências saem na serial como `compositor-verify: MISMATCH` (o conteúdo de apps WASM, de terminais executando e das Tarefas é ignorado: lê o relógio) |

Resultado (BIOS e UEFI, mesmas máquina e cenas; o "antes" é o commit `57b9121`):

| | antes | depois |
|---|---|---|
| Conjunto A, pares diferentes | 1 de 34 (e janelas fantasmas no Alt+Tab) | 0 de 34, 0 pixels |
| Conjunto B, pares fora do ruído das Tarefas | 3 de 23 (acima de 6000 px: 6066, 9974, 15 270), 9 com mais de 900 px; o Editor pintado acima das Tarefas; sombras faltando | 0 de 23 (maior diferença 1077 px, BIOS; 1294 px, UEFI, todas nos números das Tarefas) |
| Rajada sem Tarefas | 0 | 0 |
| `w20-cursor` + `w20-cursor-check` | 0 | 0 |
| `w27-flicker-check` | estável | estável |

Depois do merge com o W23-W28 (conjuntos A, B e C no BIOS, B e C no UEFI): 0 pares fora da tolerância, `w20-cursor` 0 pixels, `w27-flicker` estável, rajadas sem Tarefas e a do navegador com 0 pixels entre quadros.

Custo de quadro (QEMU/TCG sem KVM, 1280x720, `perf-trace`, `tools/perf/summ.py`; mesma imagem de teste
por cenário; o hospedeiro é compartilhado, então repetições variam uns 5-8 %):

| Cenário | antes | depois |
|---|---|---|
| ocioso | tique do relógio 0,40 ms (0 além disso) | 0,37 a 0,53 ms (0 além disso) |
| arrastar a janela, quadro de dano (a regressão de ~10 % da Tarefas: 6,3 para 6,9 ms) | 6,8 e 7,3 ms; CPU do compositor 44-45 % | 4,5, 4,7 e 4,9 ms; CPU 34-36 % |
| abrir e fechar a Calculadora, quadro de animação | 3,5 ms + 25 reconstruções de 28 ms + 12 "settle" de 12 ms; CPU 19,7 % | 5,0 a 5,5 ms, sem reconstrução; quadros de entrada de 9 a 13 ms; CPU 22,9 % |
| Tarefas visíveis (CPU), quadro do gráfico | ~2,8 ms por quadro real; CPU 20,6 % | ~3,5 ms; CPU 19,6 % |
| Snake + Tarefas visíveis, quadro estável | 13 ms (55 quadros/s); CPU 58 % | 5,2 ms (140 quadros/s); CPU 41 % |

Depois do merge, na árvore com o shell atual (Arquivos, Editor, Terminal, Navegador, i18n): arrastar uma janela 4,8 a 5,5 ms por quadro de dano; Snake + Tarefas 5,0 a 5,5 ms; rolar `kitsune://sobre` no navegador 6,4 a 8,6 ms por quadro (antes do merge, o caminho só-cliente do W24: 5 a 6 ms; o que resta é a página em si, cerca de 6 ms, mais a faixa de 8 px dos cantos de baixo, que vem do papel de parede, e o retângulo do cliente repintado por inteiro).

O arrasto ficou mais barato porque o dano é um retângulo só do tamanho das duas posições e as camadas
opacas escondem o que está embaixo; a regressão de 10 % que o commit das Tarefas trouxe (6,3 para 6,9 ms, sem causa achada)
desapareceu junto com o caminho antigo (a recomposição da cena estática). **Lacuna conhecida:** abrir e fechar uma janela sobre outra
ficou cerca de 1,5 ms mais caro por quadro (a janela de baixo é repintada, antes vinha de uma cópia em
cache), e um quadro com uma janela "viva" por cima de outras repinta as de baixo só onde o dano as toca.

## 5. Lint e supply chain

```bash
cargo lint-kernel      # clippy no alvo bare-metal, -D warnings
cargo lint-host        # clippy do core e do builder, -D warnings
cargo fmt --all -- --check
cargo deny check       # advisories, licenças, bans, fontes (deny.toml)
cargo audit
```

O kernel liga `#![warn(clippy::undocumented_unsafe_blocks)]`: com `-D warnings`,
**todo `unsafe` precisa de um comentário `// SAFETY:`** dizendo a invariante.

## 6. Plataforma de apps WASM (W13)

Projeto em [`docs/design/apps.md`](design/apps.md). O que é testado e como:

| O quê | Como | Resultado |
|---|---|---|
| Leitor de seções, manifesto, ícone, quotas | `cargo test-core` (`wasmsec` 24, `appmanifest` 43 testes) | tabela de chaves válidas/inválidas, duplicadas, acima do teto, `x-`, CRLF, LEB128 hostil, todos os prefixos e todas as trocas de um byte de um pacote sem pânico |
| Sandbox de arquivos | `appfs` (40 testes), incluindo a **tabela de caminhos hostis** (`..`, `../..`, `/../`, `a/../../b`, `//`, `\`, NUL, controle, não ASCII, `%2e%2e`, nomes longos, profundidade), um teste de propriedade (30 000 caminhos gerados: nenhum resolve fora do prefixo) e uma sequência de 20 000 operações hostis sobre dois apps que dividem um `MemFs` | nada existe fora de `/data/<id>`; um app não vê os arquivos do outro; cota e teto de descritores valem |
| Rede, ABI, instalador | `appnet` (13), `appabi` (7), `appinstall` (18) | filtro de destinos (loopback, privados, IPv6, IPs disfarçados), `check_range` sem estouro, duplicado/inválido/acima do teto/semeadura sem sobrescrever |
| Fuzz `app_manifest` | `cargo fuzz run app_manifest -- -max_total_time=660` | 57,3 M execuções, 0 falhas |
| Fuzz `app_sandbox` | `cargo fuzz run app_sandbox -- -max_total_time=660` | 2,7 M execuções (operações com caminhos em bytes crus sobre dois apps), 0 falhas |
| Quatro apps ao mesmo tempo | `w13-apps.sh` (BIOS e UEFI) | `docs/img/apps-*.png` |
| App hostil | `w13-hostile.sh` | log abaixo; `docs/img/apps-hostile-*.png` |
| Instalar/remover | `w13-install.sh` | `docs/img/apps-files.png`, `apps-start.png` |
| Heap estável | `w13-soak.sh` + `w8-heap.sh` (100 rodadas de Ola + Pintura) | BIOS: primeira amostra = última = 1 415 248 B (`last-first = 0 B`), pico de 4 797 408 B com os apps abertos; UEFI: primeira amostra = última = 1 415 248 B (`last-first = 0 B`), pico de 5 169 408 B com os apps abertos |
| CPU por app | `w13-cpu.sh` | `docs/img/apps-taskmgr.png` |

### W18: apps e gerenciamento no disco e na rede reais

| O quê | Como | Resultado |
|---|---|---|
| `VolumeFs` (AppFs sobre o VFS) | `cargo test -p kitsune_core volume` (18 testes) | os mesmos passos dão os mesmos resultados no `MemFs`; só `/apps`, `/data`, `/home` alcançáveis; persistência de dados, pacotes e remoções por remount de um `Fs3<RamDisk>`; cota exata depois do remount; volume de 1 MiB cheio: `NOSPC` e `fsck` limpo; esparsos; sequência do sandbox idêntica nos dois |
| `seed_once` | `cargo test -p kitsune_core appinstall` | uma vez só; app removido não volta, mesmo depois de remount; pacote novo entra; marcador hostil |
| Lugar Apps do Arquivos | `cargo test -p kitsune_core fileman` (11 novos) | pseudo-caminho que nunca lê o volume; seleção por id; `Enter`/`I`/`Del` e mensagens; menu sem comandos de arquivo; texto do manifesto |
| Política de rede | `cargo test -p kitsune_core appnet appmanifest` | `net_hosts`, `authorize`, cada salto de redirecionamento, `app_response` |
| Log de boot | `cargo test -p kitsune_core klog` | `dump_bounded` nunca passa do limite, mantém as linhas mais novas inteiras |
| Persistência de app (QEMU) | `w18-persist-1.sh`, depois `FS_IMG=<out1>/fs.img` com `w18-persist-2.sh` | boot 1: Notas salva `nota-1.txt` (29 B) em `/data/notes`; boot 2: `apps: 0 bundled packages installed` e a nota abre (`docs/img/w18-persist-notes.png`) |
| Lugar Apps (QEMU) | `w18-apps.sh`, `w18-wasmfile.sh` | remover/instalar/abrir, erros, menu, Propriedades do app e de `/apps/hello.wasm` (`docs/img/w18-files-apps.png`, `w18-app-props.png`, `w18-wasm-props.png`) |
| Configurações e logs entre boots (QEMU) | `w18-settings-1.sh`, depois `-2.sh` com o mesmo disco (preparo no cabeçalho do primeiro) | imagem `/papel.png`, destaque violeta, 12 h, fuso UTC-02:00 e ABNT2 voltam (`settings: loaded 120 bytes`); `fs3_inject --ls /var/log` lista `boot.log` e `syslog.txt`; `/etc/kitsune.conf` no disco (`docs/img/w18-settings-persisted.png`) |
| Rede real de app (QEMU) | `tools/nettest-server.py &`; `QEMU_NETDEV="user,id=n0,net=203.0.113.0/24,host=203.0.113.5,dhcpstart=203.0.113.15,dns=203.0.113.3"` com `w18-net.sh` (preparo no cabeçalho; app `wasm-apps/nettest`, **não** embutido) | `/hello` 26 B; redirecionamento válido seguido; redirecionamento para `10.0.2.2` recusado no salto; 404 e fora de `net_hosts` e IP privado recusados; gzip e chunked decodificados; HTTPS autoassinado: `tls: certificate check FAILED` e o app recebe erro; `127.0.0.1.nip.io`: "resolves to 127.0.0.1, a non-public address: refused"; o servidor só viu os pedidos permitidos (`docs/img/w18-net-app.png`) |
| Heap com o disco | `w13-soak.sh` (`ROUNDS=30`) + `w8-heap.sh`, build `perf-trace` | 234 amostras: primeira 1 001 312 B, última 982 416 B (sem deriva), pico de 4 364 568 B com os apps abertos |

**App hostil.** Não está no repositório: é um módulo WAT montado por um gancho **temporário**
em `kernel/build.rs` (`apps/hostile.wasm`, `fs=own`, `max_fds=4`, `mem_mib=2`) e o gancho é
revertido depois (`git checkout kernel/build.rs`). A tecla 1 a 7 dispara um ataque cada.
Serial do `w13-hostile.sh` (um app morre por vez; clock e notes seguem rodando):

```text
apps: `hostile` crashed: falta de combustivel (laco infinito?)
apps: `hostile` crashed: ponteiro invalido
[app hostile] memory.grow: -1
[app hostile] memory.grow: 1
[app hostile] sandbox: path escape refused (attempt 1)
[app hostile] open ../../etc/x: -1
[app hostile] open /data/notes/nota-1.txt: -2
[app hostile] sandbox: path escape refused (attempt 2)
[app hostile] open ../notes/nota-1.txt: -1
[app hostile] fds opened / last code: 4
[app hostile] fds opened / last code: -7
[app hostile] net_http_get without permission: -1
```

Os ataques que matam o app (laço infinito, ponteiro inválido) deixam na janela dele
"O app encerrou: <motivo>"; os demais devolvem um código de erro e o app continua.

## O que ainda não é testado

- O `kernel/` não tem testes automatizados. A migração de lógica pura para o core
  é contínua (ver [`ROADMAP.md`](ROADMAP.md)).
- As funções `osj.*` (`kernel/src/wasm/abi2.rs`), o `AppManager`, a cola do desktop e o
  transporte de rede do app (`fetch::app_get`, o slot compartilhado com o navegador) não são
  fuzzados nem têm teste unitário (são `kernel/`): a decisão que eles executam (caminhos, cotas,
  URL, `net_hosts`, resposta, manifesto, ponteiros) está em `kitsune_core` e é testada, e o
  conjunto é exercitado por boot (`w13-*.sh`, `w18-*.sh`, app hostil). Uma falha de E/S do ATA
  no meio de uma instalação (volume "envenenado" por um erro de commit) foi vista **uma vez**
  num boot de QEMU muito carregado e não se repetiu em 10 execuções seguintes (nem com 4
  processos de CPU em paralelo); o driver agora loga cada transferência que falha.
- Nenhum teste em hardware real.
- TLS real: o sandbox de QEMU não tem internet útil; os caminhos de rede são
  testados por unidade e por boot. A resolução DNS pela SLIRP funcionou neste ambiente
  (`example.com`), mas o servidor DHCP/DNS testado é sempre o da SLIRP.
- O CI (`ci.yml`) foi escrito mas ainda não rodou no GitHub; cada comando dele
  foi executado localmente.
