# Roadmap

O que o Kitsune já entrega, o que vem a seguir e por que nessa ordem. A ordem vem da auditoria
([`audit/RELATORIO.md`](audit/RELATORIO.md)): primeiro o que protege o sistema, depois o que amplia o que
ele consegue fazer, depois o polimento. Cada item tem um **critério de aceite verificável**.

## Entregue

| Área | O que o Kitsune faz hoje |
|---|---|
| Boot | Sobe do firmware BIOS ou UEFI direto para o desktop; tela de erro e serial em qualquer falha fatal |
| Robustez | GDT/TSS com pilha própria para #DF, todas as exceções tratadas, **páginas de guarda** nas pilhas, uma thread que falha morre sozinha, disco intocado se a leitura falhar |
| Scheduler | Preemptivo a 250 Hz, threads prontas/bloqueadas, CPU real por thread, compositor ocioso sem acordar à toa |
| Armazenamento | **OJFS v3**: journal de metadados, dados *copy-on-write*, extents, CRC32, `fsck` no boot, cache de blocos, `FLUSH` no disco, camada VFS única |
| Rede | `virtio-net` e NE2000, ARP/IPv4/ICMP/DHCP com renovação, DNS com cache, TCP, **TLS 1.3 com cadeia de certificados verificada** (46 raízes), hora por SNTP |
| Navegador | HTML/CSS, imagens PNG/BMP/PPM, formulários GET, favoritos, busca na página, zoom, gzip/deflate, indicador de conexão segura, JavaScript das páginas (QuickJS num processo de anel 3 por aba, DOM e eventos básicos), bloqueador de anúncios e rastreadores com chave por site |
| Apps | Plataforma WebAssembly com manifesto, permissões, cotas de CPU e memória, dados em `/data/<id>`, instalação em `/apps`; Terminal, Editor, Arquivos, Imagens, Tarefas, Registro, Ajustes, Calculadora |
| Interface | Painel superior, barra de tarefas flutuante, Apps com categorias, Busca, encaixe de janelas, 2 a 4 áreas de trabalho, configurações rápidas, central de notificações, tema claro/escuro, animações com opção de reduzir movimento |
| Idiomas | Português do Brasil e inglês, trocados ao vivo, com formatos de data e número do idioma |
| Qualidade | 2953 testes no `kitsune_core`, 17 alvos de fuzz, `unsafe` 100% documentado e imposto por lint, boot BIOS e UEFI verificado a cada mudança de kernel, `cargo deny` e `cargo audit` |

Os números e os comandos que os reproduzem estão em [`BENCHMARKS.md`](BENCHMARKS.md).

## Próximos passos, em ordem

### 1. Reiniciar e limpar uma thread que morreu
Hoje uma thread que falha morre sozinha e o resto continua. Falta **reiniciá-la** (o navegador e o app WASM
ficam indisponíveis até o reboot) e **liberar** o que ela usava (pilha, alocações, `Store` do `wasmi`, locks).
*Aceite:* matar o `fetcher` e abrir o navegador de novo funciona sem reiniciar o sistema.

### 2. HTTPS completo
A cadeia, o nome e a assinatura do handshake já são verificados. Vêm a seguir: revogação (CRL/OCSP),
*pinning* e HSTS; no navegador, POST, `<select>`/`<textarea>`, JPEG/GIF e reuso de conexão.
*Aceite:* um certificado revogado é recusado; um formulário POST envia e recebe a resposta.

### 3. Rede em hardware real
O DHCP renovável, o DNS com failover e o `virtio-net` estão provados em QEMU. Falta um driver para uma NIC de
PC (`e1000` ou `rtl8139`), interrupções da NIC e `RELEASE` no desligamento.
*Aceite:* o navegador carrega uma página usando uma NIC que existe em hardware real.

### 4. Hardware real e aceleração gráfica
Testar em UEFI real, medir o custo de VRAM, dimensionar os buffers pelo framebuffer (hoje fixos em 1080p),
adaptar à resolução e enviar à tela só o que mudou; depois, aceleração por GPU.
*Aceite:* boot e desktop num PC real com tela maior que 1080p; custo de quadro medido lá.

### 5. WebAssembly com fuel retomável
Um quadro pesado legítimo (como o carregamento de nível do DOOM) excede hoje o teto de 20 M de instruções por
chamada; o *fuel* passa a ser retomável. Também entram a escolha de app em tempo de execução e WASI sobre o OJFS.
*Aceite:* dois apps WASM abertos ao mesmo tempo, um deles hostil, sem afetar o outro.

### 6. Mais lógica testada no host
Continuar migrando decisões do kernel para o `kitsune_core`: o despacho de entrada (`desktop/input/`) e o
parse do cabeçalho do anel do DP8390.
*Aceite:* o kernel perde linhas de decisão a cada mudança e o core ganha testes.

### 7. Fuzzing contínuo
Rodar as regressões de `fuzz/regressions/` no CI e campanhas longas; fuzz do `smoltcp` e do parser de `.wasm`
via harness próprio; modelo do alocador sob Miri.
*Aceite:* um job de CI que falha se uma entrada de regressão voltar a travar.

### 8. Usuários, permissões e isolamento por processo
Em três fases (ver [`design/usuarios-seguranca.md`](design/usuarios-seguranca.md)): **(1)** contas, senhas com PBKDF2,
dono/grupo/modo nos arquivos, login e bloqueio de tela, sem ring 3; **(2)** assinatura de pacotes `.wasm`, HTTPS completo
e volume criptografado; **(3)** processos em ring 3, que levam a fronteira entre apps e kernel do WebAssembly com limites
para o hardware. Custo estimado da fase 3: 22 a 33 dias de um desenvolvedor para o MVP de um processo, 50 a 75 com os
apps portados ([ADR](audit/adr-isolamento.md)).
*Estado:* contas, login, permissões e `Secured` estão ligados ao kernel. Fase 3: programas ELF em ring 3 com
arquivos (nos direitos do usuário), argumentos e pipe, e apps WebAssembly v2 em ring 3, um processo por app
([`design/processos.md`](design/processos.md)). Processos ELF têm também janela e entrada (teclado e ponteiro), iniciam filhos (`spawn`) e já há um
**Terminal em ring 3** (`kterm`, `run kterm`). Falta: levá-lo ao dock/lançador e dar a ele `ps`/`kill`/`whoami`, mover o
Editor e o Navegador para ring 3, e rede para processos.
*Aceite:* a fase 1 prova, com testes, que um usuário não lê nem apaga o arquivo de outro; a fase 3, que um app nativo
com falha de memória não derruba o kernel nem os outros apps.

### 9. Compatibilidade: apps, web e formatos
SDK documentado e WASI sobre o OJFS; GIF e JPEG, POST, `<textarea>` e `<select>`, ZIP e codificações de texto
(ver [`design/compatibilidade.md`](design/compatibilidade.md)).
*Aceite:* uma página com GIF e JPEG mostra as imagens; um formulário POST envia e recebe a resposta; um programa em
Rust compilado para WASI roda no Terminal.

### 10. Bloqueador de anúncios e rastreadores nativo (feito em 2026-10; extras a fazer)

O navegador traz, de fábrica, um bloqueador no estilo dos "Escudos" do Brave (só a ideia: nada do código nem da
marca). **Feito:**

- **A pessoa escolhe.** Ligado por padrão; chave global em Ajustes, Rede, "Navegador", e **chave por site** no escudo
  da barra de endereço (lembrada em `~/.shields` (`/home/<nome>/.shields`)), que recarrega a página sem o filtro para o site que pedir a
  desativação.
- **Como:** lista própria embutida (hosts de anúncios e de rastreadores, trechos de caminho, seletores cosméticos),
  casada sem alocar por requisição, aplicada **antes** do `fetcher` pedir folhas, `@import`s e imagens, e na
  diagramação (imagem bloqueada não ganha caixa; caixas de anúncio escondidas por seletor). A navegação de topo nunca é
  bloqueada. Contador de itens retirados por aba no escudo.
- **Aceite:** a página `/ads` de `tools/w24-site.py` não pede nenhum recurso de anúncio ou rastreador com o filtro
  ligado e pede todos com ele desligado; o botão desliga só o site atual; testes no host e o cenário
  `tools/perf/scen/w53-shields.sh` no QEMU (ver CHANGELOG para o que foi provado).
- Ver `docs/design/tls-browser.md` §11.

**A fazer (extras, em ordem):** remover parâmetros de rastreio das URLs (`utm_*`, `fbclid`...), subir http para https
quando o site oferece, lista atualizável e formato de filtros comum, e o gancho dos scripts quando o navegador rodar
JavaScript (a fonte de scripts pergunta o mesmo `PageShield::check`).

### 11. JavaScript e YouTube com vídeo e áudio (pedido do dono do projeto; trabalho de meses, em etapas)

Meta: abrir o YouTube, tocar um vídeo com áudio. É a página mais difícil da web de hoje; ela exige, **todas**, as peças
abaixo, e nenhuma existe ainda. Ordem proposta, cada etapa com aceite próprio:

1. **Motor de JavaScript. Feito (2026-10).** O dono autorizou C fora do kernel desde que rápido, então em vez de
   QuickJS em WebAssembly no `wasmi` é o **QuickJS nativo** (C, MIT) num processo de anel 3: `run jsrun arquivo.js`
   (também `-e`, entrada padrão e `--selftest`), com `console.log`, `await` no topo, promises, erros com pilha, limite
   de memória (48 MiB) e de pilha (~3700 chamadas, `RangeError`). Provado no QEMU (`w51-jsrun.sh`): 56/56 verificações
   do teste embutido (ES2015–ES2023: classes, generators, async, regex Unicode, BigInt, Proxy, JSON, Date em UTC pela
   chamada 9 nova), script do Terminal, exceção não pega, falta de memória sem derrubar o sistema; `fib(27)` em
   ~0,3 s sob emulação TCG (~20 ms nativo no hospedeiro). Ver `docs/design/processos.md` ("JavaScript em anel 3").
   Ficou de fora: `test262`, `Intl`, módulos `import`, `setTimeout`, limite de tempo de CPU (o `Ctrl+C` mata), e a
   ligação com o Navegador (é a etapa 2).
2. **DOM e APIs da web. Parte mínima feita (2026-10).** O Navegador roda o JavaScript das páginas: um processo
   `jsrun --page` (anel 3) por aba com scripts, com a sua cópia do DOM em JavaScript, ligado ao navegador por duas
   filas de mensagens (chamadas 56 e 57). Scripts inline e externos (buscados pelo `fetcher`) em ordem,
   `DOMContentLoaded`/`load`, `document`/`querySelector(All)`/`createElement`/`appendChild`/`innerHTML`/`classList`/
   `setAttribute`/`style`, eventos com captura e bolha, clique com `preventDefault` (o link ou o envio espera o veredito
   da página), `setTimeout`/`setInterval`/`requestAnimationFrame`, `location`, `navigator`; cada mudança no DOM volta
   ao navegador e refaz o layout; 3 s por tarefa (um laço infinito é interrompido e a página segue). Provado no QEMU
   (`w52-browser-js.sh`, 19/19) com a página `/js` de `tools/w24-site.py`; ver `docs/design/tls-browser.md` §12.
   **Rede dos scripts: feita (2026-10).** `fetch` (corpo em stream com `getReader`, `Range`, respostas de vários MiB,
   `AbortController`), `XMLHttpRequest`, `sendBeacon`, `Headers`/`Request`/`Response`, `Blob`/`FormData`,
   `TextEncoder`/`TextDecoder`, `crypto.getRandomValues` (chamada 66), `structuredClone`, pelo `fetcher` (fila de
   pedidos dos scripts) e pelo bloqueador; provado no QEMU (`w55-net-js.sh`, 23/23). Ver `tls-browser.md` §13.
   **Falta para o aceite desta etapa** (um app pequeno de React/Preact aparece certo, não testado): teclado e texto
   dos campos visíveis ao script, eventos de mouse além do clique, medidas de layout, módulos, e um envio do DOM mais
   barato que o documento inteiro.
3. **Rede para o que o YouTube pede. Feita em parte (2026-10):** **cookies** (pote com `Domain`/`Path`/`Expires`/
   `Max-Age`/`Secure`/`HttpOnly`/`SameSite`, `document.cookie`, `~/.cookies`, terceiros bloqueados com o escudo
   ligado), **HTTP/1.1 keep-alive** com a sessão TLS reutilizada (até 4 conexões, 15 s ociosas; uma página de 25
   pedidos usou 3 conexões; vazão medida no TCG: http ~29 MB/s e https ~8 MB/s com virtio-net), `chunked`/
   `Content-Length`/redirecionamentos corretos, **CORS** (pré-voo, credenciais, `no-cors` opaco, `Origin`/`Referer`).
   **Falta:** `WebSocket` (`ws://`/`wss://` sobre a mesma pilha), HTTP/2 (se algum servidor exigir), cache de
   pré-voo, e vários downloads ao mesmo tempo (o `fetcher` atende um pedido por vez).
4. **Mídia. Decodificação e player feitos (2026-10).** O YouTube serve WebM com VP9 (itags 243/244/247) e Opus (251), então
   H.264 e AAC não são necessários. `run mediadec arquivo.webm` (anel 3) decodifica: demuxer WebM/Matroska
   incremental em Rust (`kitsune_core::webm`, entrada em pedaços como os fragmentos DASH, busca por cluster, fuzzado),
   **libvpx v1.15.2** só com o decodificador VP9 e **libopus 1.5.2** em ponto fixo (C, BSD-3-Clause, compilados como o
   QuickJS), saída I420 → RGBA em inteiros (BT.601/709, escala) e PCM s16 estéreo 48 kHz com pre-skip e
   `DiscardPadding` aplicados; `--md5`, `--pcm`, `--show` (toca numa janela) e um servidor de mensagens
   (`mediadec --serve`, sobre as chamadas 56/57 do `jsrun --page`, sem chamadas novas) para o player. Provado: 90/90
   quadros com MD5 **idêntico** ao libvpx do ffmpeg em 320x180, 640x360 e 1280x720, áudio com a contagem exata de
   amostras e SNR 74,5 dB contra o ffmpeg, no hospedeiro e no QEMU (`w57-mediadec.sh`). Velocidade (SSE2 nas
   transformadas, filtro de laço e convolução): 360p a ~46 fps e 720p a ~13 fps sob emulação TCG, ~1200 e ~330 fps
   nativo (a meta era 360p a 30 fps nativo). Ver `docs/design/midia.md`.
   **Player:** `<video>`/`<audio>` nas páginas (caixa de layout 300x150 ou do tamanho do vídeo, pôster, controles
   desenhados pelo navegador: play/pausa, barra de tempo clicável, tempo, volume/mudo), `HTMLMediaElement` em
   JavaScript (`media.js`: `play()`/`pause()` com promises, `currentTime`/seek, `duration`, `volume`, `muted`,
   `readyState`, `buffered`/`seekable`/`played`, `canPlayType`, `loop`, `autoplay` e todos os eventos), `src` por
   endereço baixado pelo navegador direto no decodificador (`Range` de 1 MiB, até 30 s à frente) e **Media Source
   Extensions** (`MediaSource`, `SourceBuffer` com `appendBuffer`/`remove`/`abort`, vídeo e som em buffers separados,
   `endOfStream`, `URL.createObjectURL`). Um `mediadec --serve` por elemento (no máximo 2); o **som é o relógio** (o
   contador "tocado" do mixer), quadros mostrados quando o tempo deles chega, os atrasados descartados; aba em segundo
   plano pausa. Provado no QEMU (`w58-video.sh`, 24 verificações): 440 Hz medidos na gravação com 3000,0/2001,0/
   3000,0/3000,0/2008,0 ms para tocar, seek, controles e MSE, relógio 1,001–1,008, imagem andando; sem placa toca mudo
   pelo relógio do sistema. Velocidade sob TCG: 11–13 quadros/s mostrados no 360p de 30 fps (decodificador ~65 % da
   CPU emulada), som perfeito. Ver `docs/design/tls-browser.md` §14. **Falta:** WebAudio (`decodeAudioData`), EME/DRM
   (o YouTube não exige para a maioria dos vídeos), VP8/Vorbis/H.264/AAC, tela cheia, legendas, `IN_CUE` (buscar as
   `Cues` antes de um seek longe num `src` por endereço) e velocidade de reprodução ≠ 1.
5. **Som. Feito (2026-10).** Driver **AC'97** (`-device AC97`; o `run.ps1` liga com `-audiodev dsound`), por polling,
   ~128 ms à frente; **mixer** no kernel com até 16 fluxos (4 por processo), reamostragem linear de 8–96 kHz para 48 kHz,
   volume por fluxo e do sistema, mudo, soma com saturação e o **relógio "tocado"** de cada fluxo (o que o player de vídeo
   segue); chamadas **60–65** (`audio_open/write/status/close/ctl/wait`); `play arquivo.wav`, `tone`, `beep`, `volume`;
   seção Som nos Ajustes. Provado no QEMU gravando o que o guest toca (`w56-audio.sh`): 440 Hz medidos 440,01 Hz,
   2 s medidos 1999,8 ms, silêncio exato depois, WAV de 22,05 kHz reamostrado, acorde de dois fluxos, volume 25 %
   medido 0,250, relógio com inclinação 0,998–1,017. Ver `docs/design/audio.md`. Falta: HDA/virtio-sound, provar com `dsound`
   no Windows e em hardware real.
6. **O próprio YouTube:** o player pede dezenas de APIs (Web Workers, `requestAnimationFrame`, `MutationObserver`,
   `IntersectionObserver`, Canvas, WebAudio, DRM não, mas vários recursos de segurança). Aceite: abrir um vídeo e vê-lo
   e ouvi-lo por um minuto.

Sem a etapa 1 nada do resto roda; a ordem acima é a mínima. Enquanto isso o navegador continua ganhando o que páginas
sem JavaScript pedem (itens do §10 do `tls-browser.md`).

### 12. Mais adiante: SMP, USB e som
Vários núcleos (APIC), prioridades no scheduler, USB (teclado, mouse e armazenamento) e áudio além do AC'97 (Intel
HDA, que é o que as placas novas têm; virtio-sound).
*Aceite:* cada um com sua prova (o desktop usa mais de um núcleo; um teclado USB funciona).
