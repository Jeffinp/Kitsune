# HTTPS verificado e navegador (W16, W17)

Estado: **certificado verificado, hora confirmada por SNTP, gzip/deflate, links
clicáveis e histórico** (W16); **imagens PNG/BMP/PPM, formulários GET, favoritos,
sugestões na barra, busca na página, zoom, seleção e cópia de texto, páginas internas
`kitsune://` e roda do mouse** (W17, §8). O que continua de fora está em §7.

## 1. Modelo de confiança

O navegador só mostra **"Conexão segura"** quando as três coisas aconteceram, nesta
conexão:

1. a **cadeia** que o servidor mandou foi montada até uma raiz da trust store embutida,
   com assinaturas, validade, `basicConstraints`/`keyUsage`, EKU `serverAuth`,
   restrições de nome e `pathLen` conferidos;
2. o **nome do site** (o host digitado, sem ponto final) casa com um `subjectAltName`
   da folha (curinga só como rótulo inteiro à esquerda, casa um rótulo);
3. a assinatura do `CertificateVerify` do TLS 1.3 sobre o transcript confere com a chave
   da folha (prova que o servidor tem a chave privada).

Quem faz o quê:

| Parte | Onde | Origem |
|---|---|---|
| Construção/validação de caminho, nomes, datas, restrições | `rustls-webpki 0.103.13` (ISC) | biblioteca revisada, `no_std` + `alloc` |
| RSA PKCS#1 v1.5 e PSS (SHA-256/384/512, chaves de 2048 a 8192 bits), ECDSA P-256/P-384 | `rsa 0.9`, `p256 0.13`, `p384 0.13`, `sha2` (RustCrypto, Rust puro) | idem |
| Tabela de algoritmos, limites, mapeamento de erros, trust store | `kitsune_core::tlsverify` | nosso, com 53 testes sobre certificados reais |
| Leitor DER/X.509 estrito (diagnóstico, limites, casamento de nomes independente) | `kitsune_core::x509` | nosso, sem pânico, fuzzado |
| Adaptador para o `embedded-tls` | `kernel/src/tlsv.rs` | nosso, ~200 linhas |

Por que não a feature `webpki` do `embedded-tls`: ela depende do `ring` (C e assembly,
não compila em `x86_64-unknown-none`), aceita **uma** CA e não suporta intermediárias.
Por que não criptografia própria: preferimos código revisado; escrevemos só a cola.

Importante: o `embedded-tls` trata um provedor **sem verificador** como "pule a
verificação" (`UnsecureProvider` é isso). Nosso `Provider` sempre tem verificador, então
um erro de configuração não desliga a checagem em silêncio.

Não há revogação (CRL, OCSP, grampeamento), nem *pinning*, nem HSTS, nem CT. Ed25519 e
RSA-PKCS#1 em `CertificateVerify` (proibido no TLS 1.3) não são aceitos.

## 2. Trust store

46 raízes públicas do pacote `ca-certificates` (bundle Mozilla): ISRG X1/X2, DigiCert
(Global G2/G3, Assured ID G2/G3, Trusted G4, TLS ECC P384 G5, TLS RSA4096 G5), GlobalSign
(R3, R6, R46, E46, ECC R4/R5), Google Trust Services R1/R3/R4, Amazon 1 a 4, USERTrust
RSA/ECC, COMODO RSA/ECC, Sectigo R46/E46, Microsoft RSA/ECC 2017, GoDaddy G2, Starfield
G2 e Services G2, IdenTrust, Entrust, QuoVadis 2/3 G3, SSL.com (4), Certum, HARICA (2),
T-TeleSec Class 2, Telekom Security RSA 2023. GTS Root R2, Baltimore CyberTrust e DigiCert
Global Root CA **não estão** no bundle atual do Mozilla e por isso não entram.

Arquivos:

- `kitsune_core/data/trust-store.bin`: `"OJTS1\0"`, contagem `u16`, depois `(u16 tamanho, DER)`.
  Cerca de 45 KiB no kernel.
- `kitsune_core/data/trust-store.sha256`: o SHA-256 (do DER) e o nome de cada raiz, na ordem
  do `.bin`. Um teste refaz o hash de todas as raízes e falha se o dado mudar sem o
  manifesto (e se algum root vencer, deixar de ser CA ou de carregar como âncora).
- `tools/trust-store.list`: a lista, por nome de arquivo do bundle.

**Como atualizar:** `tools/gen-trust-store.sh [dir]` (padrão
`/usr/share/ca-certificates/mozilla`, precisa de `openssl` e `python3`). A saída é
reprodutível (mesma entrada, mesmos bytes). Depois: `cargo test -p kitsune_core`, conferir o
diff do manifesto e a versão do `ca-certificates` registrada no cabeçalho dele.

`EXTRA_PEM="a.pem b.pem" tools/gen-trust-store.sh` acrescenta raízes marcadas `(EXTRA)`: é
**só para desenvolvimento** (as provas deste documento rodam atrás de um gateway que
reassina todo HTTPS) e o resultado nunca deve ser commitado.

## 3. Hora confiável

Validade de certificado depende de data, e o RTC pode estar errado. `kernel/src/clock.rs`
lê o RTC (em UTC) **uma vez** no boot, antes de existir outra thread (o par de portas do
CMOS é compartilhado com o relógio da tela) e o avança pelo timer. O `fetcher`:

1. ao subir, confirma a hora por SNTP (`kitsune_core::sntp`): `time.cloudflare.com`,
   `pool.ntp.org`, `time.google.com` (DNS próprio), e por último o **gateway**;
2. antes de uma carga HTTPS, se ainda não confirmou, tenta de novo (no máximo uma vez por
   minuto).

O pacote só vale se: modo 4, versão 3 ou 4, leap diferente de 3, stratum de 1 a 15
(stratum 0 é *kiss-o'-death* e é reportado), o *originate* repete o *transmit* que
enviamos (16 bits aleatórios), timestamps não nulos e em ordem, atraso de 0 a 5 s e data
entre 2024-01-01 e 2100. O deslocamento vale **só para a checagem de certificado**: o RTC
não é gravado e o relógio da tela não muda.

Sem resposta, usa-se o RTC; um RTC fora de 2024..2100 não é usado (erro "hora do sistema
incorreta"). Quando uma checagem de data falha e a hora não foi confirmada, a página diz
"Hora do sistema nao confirmada: confira o relogio".

Em redes que bloqueiam UDP/123 o gateway pode servir hora (`tools/sntpd.py` faz isso nas
provas).

## 4. Limites

| Limite | Valor |
|---|---|
| Certificados na cadeia | 8 |
| Tamanho de um certificado | 16 KiB |
| Handshake completo | 30 s (leitura individual: 12 s) |
| Chaves RSA | 2048 a 8192 bits |
| Corpo HTTP bruto | 256 KiB (inalterado) |
| Corpo descompactado (gzip/deflate) | 1 MiB |
| Hosts liberados com "continuar mesmo assim" | 8, só na memória, perdem-se ao reiniciar |

`Accept-Encoding: gzip, deflate`; `Transfer-Encoding: chunked` e `Content-Encoding` são
tratados por `browser::page_body` (primeiro `dechunk`, depois descompactar). O pedido agora
é HTTP/1.1 com `Connection: close` (gateways respondem 426 a HTTP/1.0). Uma codificação
desconhecida (`br`) ou um fluxo corrompido vira uma página curta explicando o problema.

## 5. Erros na tela

A barra de endereço mostra o estado da **conexão atual**:

| Estado | Barra |
|---|---|
| `https://` com cadeia válida | "Conexão segura" (verde) |
| `https://` aberto com "continuar mesmo assim" | "Certificado inválido" (vermelho) |
| `http://` | "Não seguro" (vermelho claro) |
| carregando `https://` ou página inicial | nada |

`Security` não tem variante "seguro" sem verificação: `HttpsVerified` só sai de
`Browser::loaded_with(Conn::Verified, ..)`, e o `fetcher` só devolve `Conn::Verified` com o
`Verifier` concluído (cadeia **e** assinatura).

Mensagens (todas começam com "Certificado inválido: "): expirado; ainda nao valido; nome
nao confere com o site; autoassinado, cadeia nao confiavel; cadeia nao confiavel;
assinatura invalida; autoridade invalida na cadeia; restricao da cadeia violada; algoritmo
nao suportado; hora do sistema incorreta; certificado malformado/grande demais; cadeia
longa demais. Outras falhas têm mensagem própria: DNS, conexão recusada, tempo esgotado,
falha TLS, redirecionamento de HTTPS para HTTP bloqueado.

Só o erro de certificado oferece **"Continuar mesmo assim (inseguro)"**. O clique
guarda o host (comparação sem diferenciar maiúsculas) numa lista em memória, recarrega, e o
`fetcher` pula a validação **só para esse host**, em todos os saltos da navegação. A página
fica marcada "Certificado inválido". Nada é gravado em disco.

## 5.1 Navegador

- **Links.** O motor `web` registra cada `<a href>` (até 2000 por página) em `Page::links` e uma
  caixa por palavra de texto do link em `Page::hits`; `Page::link_at(x, y)` devolve o `href`.
  O clique na janela converte para coordenadas da página (com a rolagem) e chama
  `Browser::open_link`, que resolve o `href` contra a URL da página com
  `redirect::resolve_redirect` (relativo, absoluto e `//host`; `javascript:`, `data:` e o
  rebaixamento https para http são recusados).
- **Histórico.** `Browser` guarda até 64 URLs absolutas em memória (nada em disco), com cursor:
  carregar uma página nova apaga o "avançar", recarregar a mesma não duplica, uma falha não
  entra. Alt+← e Alt+→ voltam e avançam na janela do navegador em foco.
- **gzip/deflate** (`kitsune_core::gzip`): cabeçalho completo do gzip, CRC-32 e tamanho
  conferidos, `deflate` com ou sem envoltório zlib, limite de 1 MiB descompactado.

## 6. Provas (QEMU) da W16

Neste ambiente de testes todo HTTPS de saída passa por um gateway que **reassina** os
sites com a CA "sandbox-egress-gateway-production": com a trust store de produção
`https://example.com` falha, corretamente, com "cadeia nao confiavel". As provas
`https://example.com` e `https://www.wikipedia.org` usam por isso uma trust store de
desenvolvimento com essa CA como raiz extra (`EXTRA_PEM`), e provam o caminho de
verificação de uma cadeia de 3 certificados real; **não provam** as raízes públicas
contra as cadeias públicas reais. As raízes são validadas por hash, parse e carga como
âncora nos testes. Os erros foram provados com servidores locais (`openssl s_server`, cadeias
de `tools/gen-tls-proof-pki.py`, o guest alcança o host em `10.0.2.2:porta`).

| Cenário | Serial | Tela |
|---|---|---|
| `example.com` (cadeia de 3, raiz extra) | `tls: chain verified for example.com (3 certs, root ...); handshake 60 ms` | `docs/img/browser-https-example.png` |
| `www.wikipedia.org` | `... handshake 144 ms`, 24 KB | `docs/img/browser-https-wikipedia.png` |
| cadeia local válida | `tls: chain verified for 10.0.2.2 (2 certs, root Kitsune Proof Root)` | |
| expirada | `certificate check FAILED (expirado)` | `docs/img/browser-cert-expired.png` |
| nome errado | `FAILED (nome nao confere)` | |
| autoassinada | `FAILED (autoassinado, cadeia nao confiavel)` | |
| raiz fora da store | `FAILED (cadeia nao confiavel)` | |
| continuar mesmo assim | `tls: UNVERIFIED connection ... user override`; badge vermelho | `docs/img/browser-cert-override.png` |
| RTC em 2025, sem servidor de hora | `FAILED (ainda nao valido)` e "Hora do sistema nao confirmada" | `docs/img/browser-cert-clock.png` |
| RTC em 2020, SNTP no gateway | `sntp: offset 200419735139 ms ... time now 2026-10-07 ... (confirmed)`, cadeia verificada | |
| RTC em 2020, sem SNTP | `FAILED (hora do sistema incorreta)` | |
| página local com links, `Content-Encoding: gzip` e `chunked` (servidor Python) | `fetch: GET .../gz.html`, 308 bytes gzip descompactados; clique no link; Alt+← volta ao índice | `docs/img/browser-links.png`, `browser-gzip.png`, `browser-back.png` |

Custo, medido no QEMU (TCG, ne2k, SLIRP) contra a árvore anterior (`d3b7166`,
`UnsecureProvider`):

| | antes | depois |
|---|---|---|
| ELF do kernel | 2 322 008 B | 2 694 200 B (+372 KB, dos quais ~45 KB são a trust store) |
| `.text` / `.bss` | 1 847 792 / 95 657 920 B | 2 196 482 / 95 658 456 B (BSS +536 B) |
| Fetch HTTPS a um `openssl s_server` local (SYN até o fim) | 79 ms | 74 ms |
| Handshake com verificação (2 a 3 certificados) | n/d | 60 a 144 ms |

O custo de CPU da verificação é pequeno aqui (ECDSA P-256 e RSA com e = 65537); o de
binário é o de `rustls-webpki` + `rsa` + `p256` + `p384`. O desktop ocioso segue
idêntico à baseline (`tools/verify-boot.sh`: 0 pixels, BIOS e UEFI).

## 7. O que ficou de fora

- **Navegador** (depois da W17): formulários `POST`, `<select>`, `<textarea>`, caixas de
  seleção e botões de rádio; JPEG, GIF, WebP e SVG (a imagem vira uma caixa com o `alt` e
  "formato não suportado"); JavaScript, cookies, `float`/flexbox; reuso de conexão (cada
  imagem abre uma conexão e, em HTTPS, um handshake); persistência dos favoritos (a
  interface `BookmarkStore` e o ponto único de troca existem, o armazenamento é em
  memória); seleção por caractere (é por palavra); a área de transferência tem 256 bytes
  (uma seleção longa é cortada); acentos só no campo de formulário (a fonte é ASCII: a tela
  mostra a letra sem acento, o valor enviado é UTF-8 correto).
- Revogação (CRL/OCSP), *pinning*, HSTS, Certificate Transparency.
- O RNG do handshake (W21) é um DRBG ChaCha20 sobre um pool de entropia
  ([`entropy.md`](entropy.md)). Sem `RDRAND`/`RDSEED` e sem virtio-rng ele vive de jitter de
  temporização (nota "Mixed"): o handshake espera 128 bits creditados e **recusa** se não vierem;
  numa VM totalmente determinística isso continua sendo uma estimativa, não uma garantia.
- Ed25519 em certificados de servidor.
- Reuso de conexão/sessão TLS (cada recurso abre um handshake novo).

## 8. Navegador na W17: imagens, formulários, UI e roda do mouse

### 8.1 Imagens

`<img src alt width height>` (até 200 por página no layout, **8 baixadas** por página).
O fluxo, sem travar a interface:

1. o layout reserva uma caixa cinza do tamanho declarado (`width`/`height`; só um dos
   dois usa a proporção quando a imagem já é conhecida, senão 4:3; sem atributos, 160x120);
2. o desktop pede as imagens uma por vez à thread `fetcher` (`fetch::try_post_image`):
   mesma pilha, mesmas regras de redirect, corpo de **no máximo 512 KiB**; o `fetcher`
   confere as dimensões no cabeçalho (**no máximo 2 Mpx**, antes de reservar memória),
   decodifica (`kitsune_core::image`), reduz à largura da coluna (`Image::fit`) e achata sobre
   o fundo da página, ali mesmo, fora da thread do compositor;
3. a imagem entra num cache LRU (`web::imgcache::ImageCache`, **6 MiB** de pixels, 24 entradas,
   mantido entre páginas), a página é diagramada de novo (sem reanalisar o HTML: `Doc`
   guarda o DOM) e a rolagem é preservada.

JPEG/GIF/WebP/SVG, falha de rede, status diferente de 200, imagem grande demais e a nona
imagem viram uma caixa com o texto `alt` e o motivo ("formato não suportado", "falha ao
carregar", "imagem grande demais", "limite de imagens"). `data:image/...;base64,` é decodificada
no próprio navegador (decodificador base64 puro, 64 KiB), sem rede. `<a><img></a>` é
clicável. `src` relativo resolve contra a URL da página; **https para http (conteúdo misto)
é recusado**. Memória: a prova de 100 navegações com 4 imagens novas cada (`perf-trace`)
estabiliza o heap em ~8,4 MiB (6 MiB de cache + o resto do sistema), com picos de ~15 MiB só
durante a decodificação da imagem de 1,26 Mpx.

### 8.2 Formulários GET

`<form method=get action>` com `input` de texto (`text`, `search`, `url`, `email`, `tel`,
`number`, sem tipo), `password` (bolinhas), `hidden`, `submit`/`<button>`. Foco por clique ou
Tab/Shift+Tab, caret, Backspace/Delete/setas/Home/End, Ctrl+V; Enter ou botão envia:
`action?nome=valor&...` com `application/x-www-form-urlencoded` sobre os bytes UTF-8 (a query
do `action` é trocada, o botão só entra se foi o apertado, no máximo 380 bytes). **`method=post`
mostra "formulários POST não suportados"** e não navega. `textarea` e `select` não são desenhados
(caixas de seleção e rádios existem desde a W24, §9). Como o teclado é US, as teclas `'` `` ` `` `~` `^` `"` são
*teclas mortas* nos campos (estilo US-Internacional): `'` e `c` dão `ç`, `~` e `a` dão `ã`; `'`
e espaço dão o apóstrofo; antes de outra letra saem as duas. Prova: digitar `a'c~ao` e `Jos'e`
envia `q=caf%C3%A9+a%C3%A7%C3%A3o&nome=Jos%C3%A9` e o servidor decodifica `café ação`/`José`.

### 8.3 Barra, atalhos e páginas internas

> A interface da barra, os botões e as abas desta tabela foram refeitos na W24; vale o §9.

| Item | Comportamento |
|---|---|
| Botões | voltar, avançar (cinza sem histórico), recarregar, início, buscar; estrela de favorito |
| Cursor | mão sobre link, botão de formulário, botão da barra e sugestão |
| Endereço | Ctrl+L ou clique seleciona tudo (digitar substitui); sugestões (favoritos, depois histórico; prefixo antes de substring; até 6; ↑/↓/Enter/clique; Esc fecha) |
| Favoritos | Ctrl+D ou a estrela; `BookmarkStore` (trait) com `MemoryBookmarks` (até 64); o ponto único de troca é `new_bookmark_store()` em `kernel/src/desktop/windows/instance.rs` (não existe `desktop/services/vfs.rs` na base desta frente, então nada é gravado em `/home/.bookmarks`) |
| Páginas internas | `kitsune://inicio` (a tela inicial), `favoritos` (com "[remover]"), `historico`, `sobre`: HTML gerado e diagramado pelo mesmo motor, sem rede |
| Rolagem | setas, PageUp/PageDown, Home/End e Espaço/Shift+Espaço (com o foco na página; Home/End movem o caret quando o foco é a barra), roda do mouse |
| Busca na página | Ctrl+F, destaca todas as ocorrências (a atual em laranja), Enter/Shift+Enter navega, Esc fecha |
| Zoom | Ctrl+`+`/`-`/`0`: 50, 75, 100, 125, 150, 200, 250, 300% (aritmética inteira; escala da fonte arredondada, medidas proporcionais) |
| Seleção | arrastar o mouse sobre o texto (por palavra), Ctrl+C copia (256 bytes) |
| Título | o `<title>` vai para a barra de título da janela (`NAVEGADOR - ...`) |

### 8.4 Roda do mouse (sistema todo)

O PS/2 negocia o IntelliMouse (taxas 200, 100, 80 e `0xF2`: id 3 liga pacotes de 4 bytes
com eixo Z); qualquer outra resposta mantém o pacote de 3 bytes (`ps2: mouse id 0 (no wheel)`
na serial). `Event::Mouse` ganhou `dz` (positivo = roda para o usuário = rolar para baixo; o
`mouse_move 0 0 <dz>` do monitor do QEMU tem o **sinal invertido**: `-1` rola para baixo). O
desktop entrega a rolagem à **janela sob o ponteiro**, focada ou não, sem mudar o foco:
Navegador rola 3 linhas por passo, Tarefas e Arquivos movem a seleção, Editor
move o cursor 3 linhas; Terminal e Calculadora não têm o que rolar.

### 8.5 Provas (QEMU, BIOS e UEFI)

| Cenário | Evidência |
|---|---|
| página local com PNG pequeno e grande (1400x900, reduzido para 864x555), PNG sem atributos, BMP, JPEG (caixa "formato não suportado"), `data:`, imagem como link e uma quebrada (404) | serial `img: ... -> 1400x900 (shown 864x555)`, `img: ... failed: Unsupported`; `docs/img/browser-images.png`, `browser-images-errors.png` |
| formulário GET com acentos, campo oculto e um formulário POST | o servidor recebe `q=caf%C3%A9+a%C3%A7%C3%A3o&nome=Jos%C3%A9&origem=kitsune%2F%C3%A7%C3%A3o` e responde `q = [café ação] (12 bytes UTF-8)`; `browser-form.png`, `browser-form-result.png` |
| roda: `mouse_move 0 0 -1` repetido | o navegador rola; sobre uma janela **não focada** rola ela e o foco fica onde estava; Tarefas e Arquivos movem a seleção; com a negociação desligada (gancho temporário) a serial diz `id 0` e o mouse continua movendo e clicando, sem roda |
| favoritos, sugestões, busca na página, zoom, seleção e cópia | `browser-suggest.png`, `browser-find.png` |
| 100 navegações com imagens novas (`perf-trace`) | heap: primeiro 327 KiB, platô de 8,4 MiB, sem deriva (`tools/perf/w8-heap.sh`) |
| desktop ocioso | `tools/verify-boot.sh`: 0 pixels de diferença contra a baseline, BIOS e UEFI |

### 8.6 Fuzz e limites

`fuzz/fuzz_targets/html_img_form.rs` (img com `src`/`alt`/tamanhos hostis, `data:`/base64, formulários
e edição, layout com zoom, busca, seleção, cache de imagens, modelo do navegador com favoritos e
páginas internas): 661 s e 640 s sem falha restante. Achou **um** bug, corrigido com teste e
arquivo em `fuzz/regressions/html_img_form/`: `set_page_title` cortava em 80 bytes dentro de um
caractere de vários bytes (pânico). Nenhum teto anterior mudou (corpo HTML 256 KiB, 8000 nós,
1000 regras, profundidade 40); a URL do navegador subiu de 220 para 480 bytes para caber a query
de um formulário.

## 9. Navegador na W24: texto proporcional, abas e nova interface

### 9.1 Layout com a fonte de verdade

O motor `web` não conhece fontes: mede por um *trait* (`web::metrics::TextMetrics`:
`width`, `width_q8`, `line_height`, `ascent`, `has_glyph`, sobre `Font { size, bold, italic, mono }`).
O kernel o implementa sobre `text::measure_q8` (Inter Regular e Semibold; JetBrains Mono para
`pre`, `code`, `kbd`, `samp` e `font-family: monospace`); o host testa com `FixedAdvance`, um
relógio determinístico (avanço = tamanho/2). As larguras entram no layout em 1/256 de pixel.
As respostas das métricas são saneadas (limitadas) antes de virar geometria: métricas hostis
(zero, negativas, `i32::MAX`) não estouram nem passam dos tetos.

| Item | Como é |
|---|---|
| Tamanhos | os reais da folha do agente (`h1` 2em ... `small`, `big`, `sub`/`sup`) e de `font-size` (`px`, `pt`, `em`, `rem`, `%`, palavras-chave), arredondados a pixel inteiro, vezes o zoom (50 a 300%) |
| Negrito | a face Semibold da Inter |
| Itálico | **sintetizado**: não há face itálica embarcada; `text::draw_slanted` inclina os glifos 12 graus |
| Quebra | gulosa pela largura medida; palavra mais larga que a linha é cortada por `fit_prefix`; CJK, emoji e outros alfabetos podem quebrar em qualquer ponto |
| Sem glifo | a fonte embarcada cobre Latim-1 e pontuação: o resto vira um quadro oco do tamanho do caractere (`has_glyph`), nunca um `?` |
| Alinhamento e linhas | `text-align`, `line-height` (normal, número, px, %), linha de base por trecho (`vertical-align`, `sub`, `sup`) e fusão de trechos vizinhos de mesmo estilo em um único `Cmd::Text` |
| Blocos | margens colapsam, `padding`, `border` (`solid` e afins, com `border-radius`), `background`, `width`, `max-width`, `min-height`, `display` (`flex` e `grid` degradam para bloco; `inline-block` vira em linha) |
| Listas, citações, `hr`, `pre` | marcadores `disc`/`circle`/`square`, `decimal`, alfa e romano, aninhados; `blockquote`, `hr`; `pre` e `code` em mono |
| Tabelas | colunas pelo conteúdo, `width` fixo ou em %, `colspan`, `cellspacing`/`cellpadding`, `border-collapse`, `valign`, tabelas aninhadas, legenda; sem `rowspan` (a célula vira uma linha só) |
| Formulários | caixa de texto, senha, caixa de seleção, rádio, botão de envio e `<button>`, alinhados à linha de base e desenhados no estilo do toolkit; `textarea` e `select` não existem |
| Seletores | tipo, `.classe`, `#id`, atributo (`[a]`, `=`, `~=`, `^=`, `$=`, `*=`), combinadores de descendente e filho, listas com vírgula, especificidade real; `:hover` e irmãos nunca casam (não restilizam a coisa errada). Regras indexadas pelo nome do elemento |
| Origem | folha do agente, atributos de apresentação (`bgcolor`, `align`, `width`, `border`, `cellpadding`...) e folha do autor, nesta ordem; `!important` por origem |

Tetos, nenhum mudou para pior: 8000 nós, profundidade 40, 1000 regras e 2000 seletores,
1 milhão de caracteres, 150 mil comandos, palavra de 4096 caracteres, altura 2^24, e agora um
orçamento de 4 milhões de passos de ancestral por camada de estilo.

### 9.2 Barra e indicador de segurança

A barra é uma só: voltar, avançar, recarregar (que **vira parar** enquanto carrega), campo
arredondado com o indicador, a estrela de favorito (anima ao marcar) e "+" (nova aba). Uma linha
de progresso corre sob o campo. **O cadeado e "Conexão segura" só aparecem com TLS verificado**
(`Security::HttpsVerified`); HTTP e TLS aceito sem verificação mostram um triângulo de aviso
("Não seguro", "Certificado inválido"). O clique no indicador abre um balão com o host, o emissor,
a validade e o estado da verificação. Os dados vêm de `browser::CertInfo`, resumida da folha do
certificado (`x509::issuer_cn/issuer_org`, texto limpo de controles) pelo `fetcher` e levada junto
da resposta (`Fetched.cert`, `Loaded.cert`, `browser_load`). Em repouso o campo realça o host e
apaga o resto do endereço; as páginas `kitsune://` mostram o esquema.

Sugestões: painel de vidro sob o campo, linha sob o ponteiro e linha selecionada com estados
próprios. Atalhos: Ctrl+L (endereço), Ctrl+D (favorito), Ctrl+F (busca), Ctrl+R, Ctrl +/-/0
(zoom, com uma pílula no canto), Alt+←/→, Esc (fecha ou para). Enter entrega o teclado à página.

### 9.3 Abas

Ctrl+T, Ctrl+W, Ctrl+Tab e Ctrl+PageUp/PageDown, Ctrl+1 a 8 e Ctrl+9 (a última); **no máximo 8**,
a nova abre depois da ativa e ao fechar vai para a vizinha da direita, senão da esquerda. Cada
aba tem **histórico, documento, rolagem, formulários, busca e zoom próprios**; favoritos e os
hosts liberados com "continuar mesmo assim" são da janela (`Browser::sibling`, `Rc<RefCell>`). A
faixa aparece com a segunda aba e some ao voltar a uma só (a página desliza), os títulos são cortados
com reticências, o emblema é a primeira letra do host (ou do título, para um IP) e abrir e fechar
animam (largura por *tween*, aba que sai fica um "fantasma" até acabar).

**Um só buscador.** A thread `fetcher` continua única. O desktop mantém a fila no próprio
estado: `browser_take_request` serve primeiro a aba ativa e depois as outras, **em ordem**, e
guarda o id da aba atendida (`req_tab`); o resultado volta para esse id, nunca para a aba que
estiver ativa quando chega. Parar uma aba marca o pedido em voo como cancelado e a resposta é
descartada. Imagens são baixadas só para a aba ativa (`img_inflight: (aba, chave)`) e o cache de
imagens é compartilhado e reregistrado ao trocar de aba. **Abas em segundo plano guardam só o DOM**:
a lista de desenho é descartada e refeita ao voltar.

### 9.4 Nova aba, erros, busca, menu, rolagem

* **Nova aba**: campo de busca grande, grade de favoritos com um quadrado de letra (sem
  favoritos, quatro sugestões) e os endereços recentes de todas as abas; claro e escuro.
* **Erros** (`browser::errors`): sem rede, nome não encontrado, conexão recusada, tempo
  esgotado, certificado (com a causa: de si mesmo, expirado, nome errado, emissor desconhecido,
  hora), falha de TLS, página grande demais ou cortada; cada um com título, uma frase, um glifo e
  **Tentar novamente** (recarrega o endereço navegado, não o que foi digitado depois). Erro de
  certificado mantém o "continuar mesmo assim (inseguro)", por site e por sessão.
* **Busca na página**: barra de vidro fina com contagem e anterior/próximo; destaque da seleção
  na cor de destaque do sistema.
* **Menu de contexto**: Copiar, Abrir link, Copiar endereço do link, Voltar, Recarregar e
  Adicionar/Remover dos favoritos (os que não se aplicam ficam apagados).
* **Rolagem**: roda com inércia (mola), barra que aparece ao rolar, páginas sempre claras: **a
  área da página nunca é invertida no escuro**, só a moldura segue a aparência do sistema.
* **Páginas `kitsune://`** (`browser::pages`): `favoritos`, `historico` e `sobre` são HTML e CSS
  diagramados pelo mesmo motor (sempre claros); `inicio` é a tela de nova aba nativa, que precisa
  de um campo de texto de verdade.

### 9.5 Provas

| O quê | Evidência |
|---|---|
| Moldura, abas, balão, sugestões, busca, menu, zoom e páginas internas, claro e escuro | `tools/perf/scen/w24-chrome.sh`, `w24-chrome2.sh`, `w24-internal.sh`; capturas em `docs/img/browser-w24-*.png` |
| Erros (certificado, nome, tempo esgotado) e "continuar mesmo assim" com o balão do certificado | `w24-errors.sh` (precisa de `tools/nettest-server.py` para o HTTPS autoassinado), `browser-w24-erro-cert-*.png` |
| Janela na frente da página continua na frente enquanto a página rola | `w24-overlap.sh` |
| Páginas de teste (tipografia, listas, tabelas, formulários, imagens, palavras longas, idiomas, estilos, citações, 2000 nós) | `tools/w24-site.py` e `w24-pages.sh` |
| Custo | `tools/perf/scen/w24-perf.sh` e `tools/perf/w24-perf.py` (`docs/TESTING.md`, "Navegador na W24") |

Limites conhecidos (os de CSS moderno estão no §10; JavaScript no §12): sem
`rowspan`; sem rolagem horizontal (`pre` quebra); alfabetos fora do Latim-1
(cirílico, grego, CJK, emoji) e escrita da direita para a esquerda aparecem como quadros; sem
cookies nem *cache* de HTTP; o itálico é uma inclinação do negrito/regular, não uma face.

## 10. Páginas reais: CSS externo e layout moderno

Até o §9 o navegador lia só o `<style>` da própria página e degradava `float`, `flex` e `grid` para bloco.
Para carregar páginas da internet de verdade (que quase sempre vivem de folhas externas e de layout
moderno) o motor ganhou:

**Folhas externas.** `<link rel=stylesheet href>` (menos `alternate` e `media=print`) e `@import` entram numa
fila por página (`web::sheets::SheetSet`, no máximo 12 folhas e 1 MiB de CSS). O desktop pede uma por vez
ao `fetcher` (`fetch::try_post_sheet`, depois da página e antes das imagens), as regras de cada uma entram
**antes** do `<style>` da página e depois das folhas anteriores (`Doc::add_sheet`) e o layout é refeito
a cada folha que chega. O override de certificado ("continuar mesmo assim") vale também para as
imagens e folhas do mesmo site.

**CSS.** `@media` (largura e altura do viewport, a sintaxe de intervalo `(width >= 600px)`, tema claro,
sem preferência de movimento, `hover`, pixel ratio 1; `print` nunca), `@supports` e `@layer` (o conteúdo vale como
está; `@supports not` é pulado), `var(--x, fallback)`, `calc()`, `min()`, `max()`, `clamp()` (um
`min(100%, 600px)` fica para o layout, que conhece a coluna), `vw`/`vh`/`vmin`/`vmax`/`ch`,
`light-dark()`, `color-mix()` e `env()` (escolhem um argumento), cores translúcidas misturadas com o fundo
do ancestral (e não com o branco), `;` dentro de `url(...)` e de strings, `html` sempre existe (`:root` vale em
fragmentos). Seletores: `:not()`, `:is()`, `:where()`, `:first/last/only-child`, `:nth-child()` e `:nth-of-type()`
(e os `last`), `:first/last-of-type`, `:empty`, `:link`, `:checked`, `:disabled`, `:enabled`; `:hover`, `:focus`,
`:visited` e os pseudo-elementos que não são `::before`/`::after` continuam fazendo a regra não valer. Combinadores de irmão (`+`, `~`) valem. Entidades: todas as do HTML 4.

**Layout** (`layout/flex.rs`): `display:flex` em linha ou coluna (`wrap`, `flex` / `grow` / `shrink` / `basis`, `gap`,
`justify-content`, `align-items` / `align-self`, `order`, margens `auto`, `reverse`), `display:grid`
(`grid-template-columns` com `fr`, px, %, `auto`, `repeat()` e `repeat(auto-fill, minmax())`, `gap`, `span`),
uma fileira de `inline-block` como caixas lado a lado (um entre texto continua na linha), `float` e `clear`
com o texto contornando (os floats ficam na caixa que os contém quando ela cria o seu contexto, tem `overflow`
diferente de `visible`, é um `::after{clear}` ou só tem floats), tabelas em *quirks mode* (sem doctype) sem herdar
`text-align` do `<center>`. Larguras de conteúdo vêm do mesmo medidor das tabelas (`table::content_intr`). Caixas
escondidas de propósito (`visibility:hidden`, `opacity:0`, `position:absolute` fora da tela ou recortada,
`text-indent:-9999px`, `[hidden]`) não são desenhadas; `position:absolute` e `fixed` ficam no fluxo.

**Conteúdo gerado.** `::before` e `::after` (`content` com texto, aspas e escapes; o texto toma a cor, o peso e o tamanho da regra;
caracteres da área privada de fontes de ícones são descartados; um bloco vazio com altura e fundo vira uma barra;
`::after{clear:both}` faz o elemento conter os seus floats).

**SVG** (`format::svg`, só inteiros): `<img src=*.svg>` e `data:image/svg+xml` são desenhados por um rasterizador próprio
com bordas suavizadas: `path` (todos os comandos; o arco elíptico vira a corda), `rect`, `circle`, `ellipse`, `line`,
`polyline`, `polygon`, `fill`/`stroke`/`stroke-width`, opacidades, `fill-rule`, `transform`, `style="..."`, `viewBox`,
gradientes (a primeira cor vale pelo gradiente). Sem texto, `use`, filtros, máscaras, recortes nem `<style>`. Logos
e ícones de Wikipédia, Python, Rust e HN saem certos.

**Limites.** JavaScript só o básico do §12, sem `position` de verdade (`sticky`, `fixed`, `absolute`
vão para o fluxo), sem imagens de fundo (`background-image: url()`), sem `grid-template-areas` nem linhas explícitas, sem fontes da web, sem
`rowspan`, sem cookies. Uma página SPA (BBC, por exemplo) mostra só o que o HTML do servidor traz.

**Como olhar uma página sem iniciar o sistema.** `tools/fetch-page-css.py URL pasta` baixa a página e as folhas
que ela liga; `tools/render-page.py pasta/page.html saida.png 1000 1400 --css pasta/css-*.css` roda o motor
(exemplo `kitsune_core/examples/render_page.rs`) e pinta a lista de desenho com PIL (a fonte é a DejaVu, então as
larguras diferem um pouco da Inter do sistema; a estrutura é a do motor).

| O quê | Evidência |
|---|---|
| Fila de folhas, ordem, `@import`, limites | `kitsune_core` `web::sheets::tests` |
| Flex, grid, inline-block, float, escondidos, `@media`, `var()`/`calc()`, quirks | `layout_tests::modern` |
| Seletores, `@media`, `@import`, `;` em `url()` | `css::css_tests` |
| Folha externa + flex + grid + float + `@media` no QEMU | `tools/perf/scen/w49-browser-css.sh` (página `/modern` de `tools/w24-site.py`) |
| Sites reais pela rede de verdade | `tools/perf/scen/w50-real-site.sh` (`CONTINUE=1` onde o proxy do ambiente reassina o HTTPS) |

## 11. Planejado: bloqueador de anúncios nativo

Ainda não existe; o plano está no [`ROADMAP`](../ROADMAP.md) (item 10). Resumo: ligado por padrão e **escolhido
pela pessoa**, por site (para quando um site pedir que o bloqueio seja desativado), com regras aplicadas antes do
pedido de cada subrecurso e na diagramação. O ponto único para isso é a fila de recursos da página
(`web::sheets::SheetSet` e `web::imgcache::ImageCache`): ambas passam a perguntar a um filtro (`kitsune_core`, puro
e testável no host) se o endereço pode ser pedido.

## 12. JavaScript nas páginas

Etapas 1 e 2 do item 11 do [`ROADMAP`](../ROADMAP.md). O motor é o **QuickJS** (C, nativo, num processo de anel 3:
`jsrun`, ver [`processos.md`](processos.md), "JavaScript em anel 3"). Desde a etapa 2 o Navegador roda o JavaScript
das páginas: **um processo `jsrun --page` por aba** cuja página tem scripts, ligado ao navegador por duas filas de
mensagens no kernel. Nada do JavaScript (nem do C) roda no anel 0.

### 12.1 Desenho

Por que um processo por aba, e com o DOM dentro dele: as APIs do DOM são síncronas (`getElementById`,
`textContent`, `appendChild`...) e chamadas às centenas por script; se cada uma fosse uma ida e volta ao
compositor, um script comum custaria milhares de trocas de contexto. Então o processo tem **a sua cópia do
DOM** (em JavaScript, `user-apps/jsrun/src/bin/jsrun/dom.js`), e o navegador e ele só trocam o documento inteiro e
eventos. O mesmo binário `jsrun` faz as duas coisas (`--page` é o modo do navegador), assim o motor, a libc e o
`build.rs` são um só.

| Peça | Onde | O quê |
|---|---|---|
| Coleta dos scripts | `kitsune_core::web::dom` (`parse_html_full`) | o parser guarda os `<script>` clássicos (sem `type` ou com tipo JavaScript; `module` e JSON ficam de fora), inline e `src`, em ordem; até 64 por documento e 2 MiB de texto inline |
| Busca dos externos | `web::script::ScriptSet`, `kernel/src/fetch` (`KIND_SCRIPT`), `bringup::pump_browser` | como as folhas: um por vez pelo `fetcher`, depois das folhas e antes das imagens; até 48 scripts e 4 MiB por página, cada um até 1 MiB (um script cortado é descartado) |
| Ids dos elementos | `Element::uid`, `number_elements` | cada elemento ganha um número em ordem de documento; é por ele que um clique acha o elemento no processo e que o documento devolvido mantém a identidade |
| Mensagens | `web::script::{init_message, click_message, parse_message}` | navegador → processo: JSON (`init` com a árvore, a URL, o idioma, o viewport e os scripts; `click` com o id, a posição e um número de sequência). Processo → navegador: um byte de tipo e o resto (`D` + HTML do documento com os ids em `data-kjs`, `A` + veredito de um clique, `N` + endereço) |
| Ligação no kernel | `kernel/src/userproc/page.rs`, chamadas 56 (`page_recv`) e 57 (`page_send`) | duas filas por processo (até 8 MiB e 256 mensagens esperando; mensagem de até 4 MiB). Só um processo iniciado com `spawn_page` tem a ligação; os outros recebem `ENOSYS` |
| Laço do processo | `user-apps/jsrun/src/page.c` | espera uma mensagem ou o próximo timer, roda as tarefas que venceram (cada uma com orçamento próprio), os jobs de promise depois de cada uma, e envia o documento se mudou (no máximo a cada ~40 ms de tarefas) |
| DOM, eventos, timers | `user-apps/jsrun/src/bin/jsrun/dom.js` | ver §12.2 |
| Navegador | `kernel/src/desktop/apps/browser/script.rs` | inicia o processo quando os scripts da aba ativa chegaram, entrega o `init`, aplica o documento devolvido (`Doc::replace_from_script`) e refaz o layout, manda os cliques, mata o processo quando a aba troca de página ou fecha (`Drop` do `JsProc`) |

Fluxo de uma página: o HTML chega e é mostrado como sempre (sem esperar scripts); as folhas e os scripts externos
são buscados; com todos os scripts em mãos o navegador inicia o processo e manda o `init`; o processo monta o DOM,
roda os scripts **em ordem, cada um como uma tarefa**, dispara `DOMContentLoaded` e `load`, e a cada lote de tarefas
que mudou algo manda o documento de volta. O navegador o passa pelo **mesmo parser tolerante** de uma página da rede
(o processo não é confiável), mantém as folhas externas e o `<style>` original e refaz o layout. Com scripts, o
layout registra a caixa de cada elemento (`Doc::set_track`) e `Page::node_at` acha o elemento sob o ponteiro.

**Cliques.** Um clique na página vai ao processo com o id do elemento mais interno sob o ponteiro (o menor retângulo
que contém o ponto). Os controles reagem na hora como antes (foco, caixa marcada). Um link ou um botão de envio
**espera o veredito** da página: o processo dispara o evento (captura, alvo, bolha, atributos `on*`, `return false`),
e se ninguém chamou `preventDefault()` responde `A seq 1` e o navegador segue o link ou envia o formulário. Uma página
que não responde em 1,5 s (ocupada ou morta) não segura o clique: a ação acontece assim mesmo. `javascript:` em
links é avaliado no processo; `location.href = ...`, `assign`, `replace` e `reload` viram navegação. Em páginas com
scripts os controles fora de `<form>` (um `<button>` solto, muito comum) passam a ser desenhados, num formulário
próprio que nunca é enviado.

**Limites de tempo e memória.** Cada tarefa (um script, um evento, um timer) tem **3 s**; passou disso o interrupt
handler do QuickJS a interrompe (erro não capturável, relatado como `Uncaught InternalError: interrupted` e
`jsrun: ... ran for too long and was stopped`), o timer que a iniciou é cancelado e a página continua viva. O
processo é preemptado como qualquer outro, então nem durante esses 3 s o desktop para (provado no W52: o cursor e o
hover seguem desenhados durante um `for(;;){}`). Memória: a do `jsrun` (48 MiB de JS, 64 MiB de heap). No máximo 8
processos no sistema: sem vaga, a página fica sem JavaScript (o log diz).

### 12.2 O que os scripts encontram

`window`/`globalThis`/`self`, `document` (`documentElement`, `head`, `body`, `title` que lê e escreve,
`readyState`, `getElementById`, `getElementsByTagName/ClassName/Name`, `querySelector(All)`, `createElement`,
`createTextNode`, `createDocumentFragment`, `createComment`, `cookie` em memória, `write` que acrescenta ao
`body`), nós (`appendChild`, `insertBefore`, `removeChild`, `replaceChild`, `remove`, `append`, `prepend`, `before`,
`after`, `replaceWith`, `cloneNode`, `contains`, `textContent`, `childNodes`, `children`, irmãos e pais), elementos
(`getAttribute`/`setAttribute`/`removeAttribute`/`toggleAttribute`, `id`, `className`, `classList`, `style` como
objeto que escreve o atributo, `dataset`, `innerHTML`/`outerHTML`/`insertAdjacentHTML` com um parser de fragmentos
próprio, `innerText`, `hidden`, `value`, `checked`, `disabled`, `href`/`src` e afins, `matches`, `closest`, `click()`),
seletores (tipo, `#id`, `.classe`, atributos com `= ~= |= ^= $= *=`, combinadores ` > + ~`, listas, `:first/last/only-child`,
`:nth-child()` e afins, `:not()`, `:is()`, `:where()`, `:has()`, `:empty`, `:root`, `:checked`, `:disabled`, `:scope`),
eventos (`addEventListener` com `capture`/`once`, `removeEventListener`, `dispatchEvent`, `Event`, `CustomEvent`,
`MouseEvent`, `KeyboardEvent`, `preventDefault`, `stopPropagation`, propriedades `onclick` etc. e atributos `on*`),
`setTimeout`/`setInterval`/`clear*`, `requestAnimationFrame` (~60 por segundo), `queueMicrotask`,
`requestIdleCallback`, `location` (partes da URL, `hash`, navegação), `navigator` (`userAgent` =
`Mozilla/5.0 (X11; Kitsune x86_64) Kitsune/<versão>`, `language`), `URL` e `URLSearchParams`, `localStorage` e
`sessionStorage` em memória, `history` (sem efeito), `matchMedia` (nunca casa), `getComputedStyle` (só o estilo em
linha), `atob`/`btoa`, `alert`/`confirm`/`prompt` (vão para o log), e `MutationObserver`, `IntersectionObserver` e
`ResizeObserver` que não observam nada. `console.log` sai no log do sistema como `[jsrun:PID] ...`.

**Ainda não** (etapa 2 mínima): `fetch` e `XMLHttpRequest` (rejeitam), cookies de verdade, módulos (`type=module`,
`import`), Web Workers, canvas, teclado e texto digitado nos campos (o script não vê o que se digita, e `input`/`change`
não disparam), eventos de mouse além do clique, medidas de layout (`getBoundingClientRect`, `offsetWidth`... são 0),
estilo computado de verdade, `<link>` e `<img>` acrescentados por script depois do início (as imagens novas são
buscadas; folhas novas não). Os scripts rodam todos depois do documento inteiro (como `defer`), não no meio do
parse; `document.write` acrescenta ao fim. O documento volta inteiro a cada mudança: barato em páginas pequenas,
caro numa SPA grande (cada envio é um parse e um layout). Os scripts só começam na aba ativa.

### 12.3 Provas

| O quê | Evidência |
|---|---|
| Parser guarda os scripts, ids, `replace_from_script` (folhas externas e `<style>` mantidos, entrada hostil), caixa por elemento, `ScriptSet`, mensagens e o clique que espera o veredito | `kitsune_core` `web::script::tests` (18 testes) |
| O DOM em JavaScript no hospedeiro | `cargo run -p kitsune_core --example script_init -- pagina.html URL nome.js=arquivo.js` gera o `init`; o `jsrun` compilado para o hospedeiro com um laço de teste (não versionado) rodou a página `/js` com cliques |
| A página `/js` de `tools/w24-site.py` no QEMU (BIOS, 256 MiB) | `tools/perf/scen/w52-browser-js.sh`: **19/19**. Script externo buscado, processo iniciado, scripts em ordem, `DOMContentLoaded`, `load`, `createElement`/`appendChild`/`insertAdjacentHTML`/seletores, `setTimeout`, `setInterval`, `requestAnimationFrame`, promise; o título da janela, o `h1`, a lista e um parágrafo mudam na tela; dois cliques no botão contam e trocam a classe (o fundo muda); o link com `preventDefault` não navega; o `javascript:` roda; o `for(;;){}` é interrompido (orçamento de 3 s por tarefa), o desktop segue desenhando e o terceiro clique funciona |
