# Mídia: WebM (VP9 + Opus) em anel 3

Etapa 4 do item 11 do [ROADMAP](../ROADMAP.md) ("YouTube com vídeo e áudio"), parte de **decodificação**. O YouTube
serve vídeo VP9 (itags 243 = 360p, 244 = 480p, 247 = 720p...) e áudio Opus (itag 251) em contêiner **WebM**, em
fragmentos DASH (um segmento de inicialização e depois clusters). Com VP9 + Opus + WebM não é preciso H.264 nem AAC.
Esta etapa entrega o caminho "bytes do arquivo → quadros RGB e PCM" num processo de anel 3; o resto do player
(Media Source Extensions, `<video>`, sincronia, a rede do JavaScript e o driver de som) são outras peças.

## Peças

| Peça | Onde | O quê |
|---|---|---|
| Contêiner | `kitsune_core::webm` (`format/webm.rs` + `webm/`) | demuxer EBML/Matroska incremental em Rust, `no_std` + `alloc`, `forbid(unsafe)`, testado no host e fuzzado (`webm_demux`) |
| Vídeo | `user-apps/mediadec/vendor/libvpx` | **libvpx v1.15.2**, só o decodificador VP9 (BSD-3-Clause), C genérico mais os seus intrínsecos SSE2 (C), sem assembly, sem threads |
| Áudio | `user-apps/mediadec/vendor/opus` | **Opus 1.5.2**, só o decodificador (BSD-3-Clause), em **ponto fixo** (`FIXED_POINT`, `DISABLE_FLOAT_API`) |
| Configuração | `user-apps/mediadec/csrc/vpx`, `csrc/opus/config.h` | os cabeçalhos que o `configure` do libvpx gera (`generic-gnu`), ajustados à mão; o `config.h` do Opus escrito à mão |
| Cola C | `csrc/media.c`, `csrc/media.h` | invólucros dos dois decodificadores (só inteiros e ponteiros na interface) |
| Cor | `csrc/yuv.c` | I420 → RGBA em inteiros (SSE2), com escala |
| Convolução | `csrc/convolve_sse2.c` | a predição inter de 8 taps do VP9 em intrínsecos SSE2 (a do libvpx é assembly) |
| libc | `csrc/libc` | o mínimo que os dois pedem: `str*`, `qsort`, `snprintf`/`vsnprintf`, `setjmp`/`longjmp`, `abort` |
| Programa | `user-apps/mediadec/src/bin/mediadec.rs` + `mediadec/` | `ffi.rs` (invólucros seguros), `pipeline.rs` (WebM → quadros/PCM), `decode.rs` (modo arquivo), `serve.rs` (servidor de mensagens), `heap.rs` (`malloc` do C), `sys.rs` |
| Build | `user-apps/mediadec/build.rs`, `csources.rs` | clang freestanding como o `jsrun` (`--target=x86_64-unknown-none-elf -ffreestanding -nostdlibinc -O2 -msse2 -fno-pic ...`); a lista de arquivos C é uma só para o anel 3 e para o verificador do hospedeiro |
| Kernel | `kernel/build.rs`, `kernel/src/userproc.rs`, `userproc/page.rs` | `mediadec` embutido (`USER_CRATES`), pilha de 1 MiB (`BIG_STACK`), heap de até 128 MiB (`BIG_HEAP`), `userproc::spawn_media` |
| Verificador | `tools/media/mediacheck`, `check-host.sh` | o mesmo C e o mesmo pipeline (`#[path]`) compilados no hospedeiro, comparados com o ffmpeg |

## O demuxer (`kitsune_core::webm`)

* **Incremental.** `Demuxer::push(bytes)` aceita pedaços de qualquer tamanho; `poll()` devolve `Event::Header` (depois
  dele `info()` e `tracks()` valem) e um `Event::Packet` por quadro, em ordem do arquivo; `Ok(None)` pede mais bytes.
  O fatiamento não muda nada: o teste empurra o arquivo byte a byte, em pedaços primos e inteiro, e compara.
* **O que lê.** Cabeçalho EBML (`DocType` `webm` ou `matroska`), `Segment` e `Cluster` de tamanho conhecido **ou
  desconhecido** (transmissões ao vivo e DASH: um cluster sem tamanho termina no primeiro id que não pode ser filho
  dele), `Info` (escala de tempo, duração), `Tracks` (número, tipo, codec, `CodecPrivate`, tamanho do vídeo, taxa e
  canais do áudio, `CodecDelay`, `SeekPreRoll`, `ContentEncodings` marcada), `Cues`, `Timecode`, `SimpleBlock` e
  `BlockGroup` (`Block`, `BlockDuration`, `ReferenceBlock`, `DiscardPadding`). O resto (`SeekHead`, `Tags`, `Void`...)
  é pulado **sem ser guardado** (um contador consome os bytes conforme chegam). O `OpusHead` (RFC 7845) também.
* **Pacote.** Trilha, tempo em ns (`(tempo do cluster + deslocamento do bloco) × TimecodeScale`, saturado), keyframe,
  duração, `DiscardPadding` em ns, posição do cluster e os bytes.
* **Busca por cluster.** `cluster_for(t)` acha nas `Cues` o cluster a partir do qual mostrar `t`; `seek(offset)` joga
  fora o que estava no buffer e passa a ler os próximos bytes como começando naquele offset do arquivo, no nível do
  segmento, com cabeçalho e cues mantidos. É o que um player DASH faz: baixa o init, pede o cluster pela faixa de bytes.
* **Entrada hostil.** Sem pânico e sem memória sem limite: bloco até 16 MiB, `Info`/`Tracks` até 1 MiB, `Cues` até
  4 MiB (maior é pulado: busca sem índice), 16 trilhas, 65 536 cues, 64 MiB pendentes, aninhamento fixo (segmento,
  cluster, grupo), filho maior que o pai é erro, número de trilha 0 ou repetido é erro. Depois de um erro o demuxer
  fica falho até `seek`/`reset`. **Sem suporte:** blocos com *lacing* (vídeo WebM nunca usa; o Opus em WebM também
  não) são pulados e contados.
* **Provas.** `cargo test -p kitsune_core webm` (14 testes): o arquivo `testdata/small.webm` (320x180 VP9 + Opus, 1 s,
  17 KiB, do ffmpeg) dá **os mesmos 80 pacotes do `ffprobe`** (tamanho, keyframe, tempo), cabeçalho e cues certos, a
  busca por cue recomeça num keyframe no tempo da cue; fluxos sintéticos com tamanhos desconhecidos, grupos com
  `DiscardPadding`/`ReferenceBlock`/`BlockDuration`, bloco com lacing, dois segmentos seguidos (dois cabeçalhos),
  `DocType` errado, tamanhos hostis, elemento pulado de 40 MiB e 300 arquivos corrompidos. Fuzz: alvo `webm_demux`
  (semente: o `small.webm`), 10 minutos, **2,93 milhões de execuções (~4900/s), nenhuma falha**, 939 arestas de
  cobertura.

## Os decodificadores

**VP9 (libvpx).** O que se vendorizou é o que um programa só de decodificação liga (achado ligando um programa de
prova contra a biblioteca inteira com mapa do ligador): 43 arquivos C e os cabeçalhos que eles incluem, mais três
arquivos de intrínsecos SSE2 do próprio libvpx (`inv_txfm_sse2.c`, `loopfilter_sse2.c`, `vp9_idct_intrin_sse2.c`),
sem nenhuma mudança (~1,5 MB; o Opus, ~1,4 MB). Os cabeçalhos gerados (`vpx_config.h`, `vp9_rtcd.h`, `vpx_dsp_rtcd.h`, `vpx_scale_rtcd.h`, `vpx_version.h`)
saíram do `configure --target=generic-gnu --disable-vp8 --disable-vp9-encoder --disable-multithread
--disable-runtime-cpu-detect ...` e foram editados: sem `pthread.h`/`unistd.h`, sem PIC, sem "OS support",
**limite de quadro 1920x1088** (`CONFIG_SIZE_LIMIT`: um fluxo hostil não pede um quadro gigante), e as entradas das
transformadas, do filtro de laço e da convolução apontadas para as versões SSE2. Perfil 0 (8 bits,
4:2:0) é o que se mostra; outros perfis decodificam mas o quadro é recusado pela cola (o YouTube usa perfil 0 fora do
HDR). Um quadro corrompido faz o libvpx sair por `longjmp` com erro (o `setjmp`/`longjmp` é o da nossa libc, o layout
do musl); o próximo keyframe recupera.

**Opus (libopus).** Decodificador em ponto fixo: SILK, CELT e híbrido, mono ou estéreo, 2,5 a 60 ms (120 ms por
pacote), sempre entregue como **estéreo s16 a 48 kHz** (um fluxo mono sai duplicado). Inteiros puros: o mesmo código
dá os mesmos bytes no hospedeiro e no anel 3 (é o que o cenário W57 confere). Ganho do `OpusHead` aplicado. Família de
mapeamento 0 só (5.1 precisaria do decodificador multistream).

**Aparar o áudio.** O começo tem o `pre-skip` do `OpusHead` (312 amostras no libopus) que não é som; o fim do último
pacote tem o `DiscardPadding` do bloco. O pipeline tira os dois, então a contagem de amostras é exata (3 s = 144 000 por
canal, igual ao ffmpeg).

**Cor e escala (`yuv.c`).** Matriz BT.709 quando o fluxo diz BT.709 (ou SMPTE-240/BT.2020), BT.601 no resto; faixa de
estúdio (Y 16..235) a menos que o fluxo diga faixa cheia. Coeficientes Q13, somas em 32 bits: 8 pixels por vez em SSE2
(`_mm_madd_epi16`), a cauda escalar faz a mesma conta. Fica a **no máximo 1 nível** da fórmula exata em ponto
flutuante (conferido pelo `mediacheck` no primeiro quadro de cada arquivo); contra o swscale do ffmpeg a diferença
média é 1,3–1,7 níveis (ele interpola o croma, aqui ele é repetido). Escala para o tamanho da janela pelo **vizinho
mais próximo** em inteiros (centros alinhados): cada linha de origem é convertida uma vez, os pixels escolhidos por uma
tabela de colunas e uma linha de saída igual à anterior é cópia. Saída RGBA (bytes R, G, B, 255), o formato da janela.

**Fronteira Rust/C.** Como no `jsrun`: só inteiros e ponteiros (o Rust do anel 3 é *soft-float*, o C usa SSE2); o
`malloc` do C é o alocador do SDK (`heap.rs`, cabeçalho de 16 bytes). `memcpy`/`memset` vêm do `compiler_builtins`.

## O programa

```text
run mediadec ARQ.webm [--info] [--frames N] [--md5] [--pcm SAIDA.raw] [--rgb LxA] [--show]
run mediadec --serve-test ARQ.webm
mediadec --serve                 (iniciado pelo kernel: userproc::spawn_media)
```

* Lê o arquivo em pedaços de 64 KiB e imprime a duração, as trilhas (as que não decodifica marcadas), resolução,
  canais e pre-skip; no fim, quadros e tempo de decodificação de vídeo (**fps**), pacotes e amostras de áudio, tempo
  total, erros e blocos pulados.
* `--md5`: `frame N T ms MD5` por quadro (os planos Y, U, V juntos, sem folga: o que o `ffmpeg -f framemd5` usa) e
  `audio md5 ...` do PCM inteiro. `--pcm`: grava o PCM (s16le estéreo 48 kHz). `--rgb LxA`: converte cada quadro para
  RGBA naquele tamanho e mede. `--show`: abre uma janela e toca o vídeo no tempo dos quadros (sem som: o driver é outra
  peça). `--info`: só o cabeçalho. `--frames N`: para depois de N quadros.
* Pacote que não decodifica é contado e relatado (`video packet did not decode (error 7: ...)`); o processo termina
  sozinho, status 1.

## O servidor de mensagens (`--serve`)

Para o player futuro do navegador. `userproc::spawn_media(cred)` inicia `mediadec --serve` com o **mesmo elo** de
mensagens do `jsrun --page` (chamadas 56 `page_recv` e 57 `page_send`; o navegador usa `page_post`/`page_take`): **não
há chamada nova**. Cada mensagem é uma chamada; o primeiro byte diz o tipo; números em little endian.

| Sentido | Byte | Nome | Conteúdo |
|---|---|---|---|
| → | `0x01` | DATA | bytes do WebM (init, depois clusters), em ordem |
| → | `0x02` | SEEK | `u64` tempo (ns); resposta SEEKPOS; daí em diante o que vem antes do tempo é descartado |
| → | `0x03` | RESTART | `u64` offset do arquivo, `i64` tempo (ns): os próximos DATA começam nesse offset (um cluster que o player achou) |
| → | `0x04` | SIZE | `u32` largura, `u32` altura dos quadros enviados (0, 0 = o tamanho do vídeo; no máximo 1280x800) |
| → | `0x05` | END | sem mais dados por ora: envia tudo o que dá e responde END |
| → | `0x06` | QUIT | termina |
| ← | `0x81` | HEADER | `u64` duração (ns), `u32` largura, `u32` altura, `u8` tem vídeo, `u8` tem áudio, `u8` canais, `u8` 0, `u32` 48000 |
| ← | `0x82` | FRAME | `i64` tempo (ns), `u32` largura, `u32` altura, `largura × altura` pixels RGBA |
| ← | `0x83` | AUDIO | `i64` tempo (ns), `u32` amostras por canal, amostras s16le estéreo intercaladas |
| ← | `0x84` | SEEKPOS | `u64` offset do cluster a mandar (`u64::MAX`: sem índice) |
| ← | `0x85` | END | `u32` quadros, `u32` pacotes de áudio enviados desde o início |
| ← | `0x8F` | ERROR | `u32` código (1 contêiner, 2 vídeo, 3 áudio, 4 mensagem, 5 memória), texto UTF-8 |

Controle de fluxo: com 8 MiB esperando na fila do navegador o `page_send` devolve `EAGAIN` e o servidor espera (4 ms)
e tenta de novo, sem ler mais entrada. Um quadro de 1280x720 é 3,7 MB, abaixo do limite de 4 MiB por mensagem.
`--serve-test` dirige o mesmo `Server` por um elo em memória (quadros 160x90, o arquivo em mensagens de 32 KiB, END,
SEEK para 1 s, o resto do arquivo) e confere o que voltou; o verificador do hospedeiro roda o mesmo teste.

## Provas

**No hospedeiro** (`tools/media/gen-test-media.sh M && tools/media/check-host.sh M`; arquivos do ffmpeg com
`libvpx-vp9` e `libopus`, 3 s, 30 fps, keyframe a cada segundo, tons estéreo):

| Arquivo | Quadros com MD5 idêntico ao `framemd5` do ffmpeg (libvpx) | Áudio | `--serve-test` |
|---|---|---|---|
| 320x180 (100 kb/s) | **90/90** | 144 000 amostras exatas, SNR **74,5 dB** contra o decodificador do ffmpeg | PASS |
| 640x360 (300 kb/s) | **90/90** | idem | PASS |
| 1280x720 (1 Mb/s) | **90/90** | idem | PASS |

O decodificador de vídeo é **bit a bit** o do ffmpeg/libvpx. O de áudio em ponto fixo difere do de ponto flutuante
em ~74 dB de SNR (o limite pedido era 40 dB). Um arquivo com os clusters corrompidos (`bad.webm`) dá erros de pacote e
nenhuma queda; um cortado no meio (`cut.webm`) decodifica o que tem.

**No QEMU** (cenário `tools/perf/scen/w57-mediadec.sh`, BIOS, 256 MiB, TCG; `VERDICT: PASS` no build genérico e no
build com SSE2): os três arquivos baixados pelo `wget` do Terminal e decodificados por `run mediadec X.webm --md5` dão
**90/90 quadros com MD5 idêntico ao `framemd5` do ffmpeg** e o **MD5 do PCM igual ao do hospedeiro** (144 000 amostras
por canal): o decodificador de áudio em ponto fixo dá os mesmos bytes no anel 3 e no hospedeiro, onde o SNR contra o
ffmpeg é 74,5 dB. `--serve-test` PASS (90 quadros 160x90, 144 000 amostras, a busca para 1 s volta o offset do cluster
certo e o primeiro quadro depois dela é o de 1000 ms). `cut.webm` decodifica os 30 quadros que tem; `bad.webm` dá 75
pacotes que não decodificam ("Failed to decode tile data", "Keyframe / intra-only frame required...") e termina com
status 1; todo processo termina sozinho, nenhum é morto, e as execuções seguintes (`--info`, `--rgb`, `--show`)
funcionam. `--show` toca o vídeo numa janela (captura `d-show.png`).

## Velocidade

Só a decodificação de vídeo (o tempo dentro do `poll` que devolve um quadro: contêiner + VP9), um fio, os arquivos de
teste de 3 s (90 quadros, keyframe por segundo). "Nativo" é o mesmo C no hospedeiro deste ambiente (clang `-O2
-msse2`, `mediacheck`), que é compartilhado com outros processos: os números variam ~±20 % de uma execução para outra.

| Versão | 320x180 | 640x360 | 1280x720 |
|---|---|---|---|
| C genérico do libvpx, nativo | ~1300–1800 fps | ~570–690 fps | ~125–155 fps |
| C genérico, **QEMU TCG** (`w57`, anel 3) | 79 fps | 34,6 fps | 8,8 fps |
| + intrínsecos SSE2 (transformadas, filtro de laço, convolução), nativo | ~2800–4000 fps | ~1100–1300 fps | ~300–360 fps |
| + intrínsecos SSE2, **QEMU TCG** (`w57`, anel 3) | **91 fps** | **45,7 fps** | **13,0 fps** |

* **Meta (360p a 30 fps nativo): passada com folga de ~35×.** Mesmo sob emulação TCG, sem KVM, o 360p decodifica a
  ~46 fps, acima do tempo real; o 720p fica em ~13 fps sob TCG (em hardware real ou com KVM, ~300 fps).
* O que entrou para isso: as transformadas inversas (`vpx_idct*`, `vp9_iht*`) e o filtro de laço (`vpx_lpf_*`) usam os
  intrínsecos SSE2 **do próprio libvpx** (C, sem assembly); a convolução de 8 taps da predição inter (cujo SSE2 no
  libvpx é assembly) foi reescrita em intrínsecos em `csrc/convolve_sse2.c`, com a mesma aritmética. Todas são bit a
  bit iguais ao C: o MD5 de cada quadro continua igual ao do ffmpeg.
* O áudio custa ~0,3 s por 3 s de som sob TCG (~10 % de um núcleo emulado); nativo, ~10 ms.
* A conversão para RGBA (com escala para 1280x720) custa ~1–1,6 ms por quadro nativo e ~31 ms sob TCG (era ~140 ms
  com a escala bilinear escalar).
* O `total` impresso pelo `mediadec --md5` inclui o MD5 de cada quadro em Rust (sem SIMD), que sob TCG custa mais que
  a decodificação; sem `--md5` o total fica perto da soma de vídeo e áudio.

## O que falta

* Media Source Extensions, `<video>`/`<audio>`, sincronia áudio/vídeo e o player do navegador (que usará
  `spawn_media`); o som (driver de áudio) é outra peça.
* VP9 perfil 2 (10 bits, HDR), Opus multicanal (família 1), AV1, H.264/AAC.
* Threads no decodificador (o VP9 suporta por tiles; um processo tem um fio hoje).
* Busca sem `Cues` (o player precisaria do índice `sidx`/Cues do DASH; sem ele, ler do começo).
* Blocos com *lacing*, trilhas com `ContentEncodings` (criptografia/compressão).
