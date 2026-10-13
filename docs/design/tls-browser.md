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
tratados por `browser::page_body` (primeiro `dechunk`, depois descompactar). O pedido é
HTTP/1.1 e, desde o §13, a conexão (e a sessão TLS) fica aberta para os próximos pedidos ao mesmo
host (keep-alive; antes era `Connection: close`). Uma codificação
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
`rowspan`. (Cookies: §13.) Uma página SPA (BBC, por exemplo) mostra só o que o HTML do servidor traz.

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

## 11. Bloqueador de anúncios e rastreadores nativo

Item 10 do [`ROADMAP`](../ROADMAP.md), no estilo dos "Escudos" do Brave (só a ideia; nada do código nem da marca).
**Ligado por padrão e escolhido pela pessoa**: chave global em Ajustes, Rede, "Navegador" (`adblock=1|0` em
`/etc/kitsune.conf`) e chave **por site** no escudo da barra de endereço (lista em `~/.shields` (`/home/<nome>/.shields`), um site por
linha, o domínio registrável: desligar em `www.exemplo.com` vale para `exemplo.com` e seus subdomínios). A lógica é
pura e testada no host: `kitsune_core::shields` (`browsing/shields/`).

### 11.1 O que se decide

| Regra | Arquivo (`shields/data/`) | Como casa |
|---|---|---|
| Hosts de anúncios e de rastreadores | `ad_hosts.txt`, `tracker_hosts.txt` | o host da requisição ou qualquer pai dele (nunca o TLD sozinho); só para requisições de **terceiros** (o "site" do pedido difere do da página: últimos dois rótulos, três sob `co.uk`, `com.br`...; IP é o próprio site) |
| Trechos de caminho | `ad_paths.txt`, `tracker_paths.txt` | subcadeia do caminho e da query, sem distinguir maiúsculas, para qualquer requisição (começam em `/`: `/ads/banner` não casa `/uploads/banner`) |
| Cosmético | `cosmetic.css` | até 32 seletores genéricos (`.adsbygoogle`, `[id^="div-gpt-ad"]`...) com `display:none !important`, somados às regras do documento (`Doc::hide_matching`) |

Sem alocar por requisição: o host é posto em minúsculas num buffer de pilha e as listas de hosts, **ordenadas**, são
buscadas em `O(log n)` no próprio texto embutido (`include_str!`); um teste mantém os arquivos ordenados, únicos,
em minúsculas e sem linhas vazias. A **navegação de topo nunca é bloqueada** (só subrecursos passam por
`classify`; `data:` e o que não é http(s) nunca são bloqueados). As listas são **próprias do projeto** (ver
`THIRD-PARTY.md`: nada copiado de EasyList, uBlock, Disconnect etc.); as duas entradas `*.invalid` existem para a
página de teste.

### 11.2 Onde entra no pipeline

* `PageShield` (um por aba): `on`, o host da página, o `Tally` (`ads`, `trackers`, `hidden`; o número do escudo é a
  soma) e a lista (até 64) dos endereços bloqueados, que também serve para contar cada endereço uma vez só.
  Construída em `browser_load` com `PageShield::decide` (chave global ligada e site não liberado).
* **Folhas**: `SheetSet::begin_with` / `finish_with` não enfileiram folha bloqueada nem `@import` bloqueado.
* **Imagens**: no registro do layout (`layout_browser`, `register`), cada `src` bloqueado vai para
  `ImageCache::block` (nunca é pedido, não gasta as 8 vagas da página) e o layout é refeito: `ImgState::Blocked`
  **não ganha caixa nenhuma** (o `ImgRef` continua na lista para os índices baterem).
* **Cosmético**: `hide_ads` roda uma vez, ao carregar; o que está dentro de uma caixa escondida não é pedido
  (o layout nem o vê) e conta uma vez.
* **Scripts**: a fonte de scripts pergunta o mesmo `PageShield::check(url)` antes de pedir o `.js`; os pedidos que os
  scripts fazem (`fetch`, `XMLHttpRequest`, `sendBeacon`) também (§13): bloqueado é um erro de rede para o script
  (`TypeError`) e conta no escudo (`shields: blocked <ad|tracker> fetch <url>`).
* **Cookies de terceiros**: com o escudo ligado, um pedido de subrecurso a outro site não leva nem guarda cookies (§13).
* Cada bloqueio vai para a serial (`shields: blocked <ad|tracker> <sheet|image|import> <url>`, `shields: page <host>
  on=.. ads=.. trackers=.. hidden=.. total=..`, `shields: off|on for <site>`), que é o que o cenário QEMU lê.

### 11.3 Interface

Um **escudo** dentro do campo de endereço, à esquerda da estrela, em páginas http(s) já carregadas: verde com o
número de itens retirados quando filtra e há algo, cinza quando filtra sem achar nada, cinza riscado quando está
desligado. O clique abre um balão com a **chave "Bloquear neste site"** (clicar na linha inteira; desligar ou ligar
**recarrega a página** com ou sem o filtro, para um site que peça a desativação), as três contagens (anúncios,
rastreadores, caixas escondidas) e uma frase. Com a chave global desligada em Ajustes o escudo aparece riscado e a
chave do balão fica apagada. Esc, clique fora, troca de aba e página nova fecham o balão. Strings em
`web.shield.*`, `web.notice.shield_*`, `settings.net.adblock*` (pt e en).

### 11.4 Limites

* O que mudou na chave global só vale para a **próxima** carga. A lista de sites guarda até 64.
* Os subrecursos filtrados são folhas, imagens, scripts e os pedidos dos scripts. Sem `iframe`, fontes da web nem
  `background-image`; `WebSocket` não existe ainda.
* Foram feitos os itens "domínio, caminho e seletor" e o contador. **Não** foram feitos: remover `utm_*`/`fbclid`
  das URLs, subir http para https, listas atualizáveis e o formato de filtros comum (a lista é um arquivo de dados
  fixo na imagem). Mais de 64 imagens bloqueadas numa página ficam como caixa cinza sem carregar.
* O "site" é uma heurística sem a lista de sufixos públicos; sufixos incomuns (`*.pvt.k12.ma.us`) podem contar
  dois sites de uma mesma organização como um só ou o contrário.
* Regras de caminho valem também para o próprio site (um anúncio próprio ainda é anúncio); por isso são poucas e
  específicas.

### 11.5 Provas

| O quê | Evidência |
|---|---|
| Listas (ordem, sem duplicata, nenhum TLD), busca binária, hosts e subdomínios, minúsculas/porta/usuário/ponto final, quase-acertos, esquemas, regra de terceiros, caminhos, sites, entradas hostis | `kitsune_core` `browsing::shields::tests` |
| Contagem uma vez por endereço, log limitado, `decide`, lista de sites (arquivo danificado, limite, ida e volta) | idem |
| Imagem bloqueada sem caixa e sem pedido, folhas e `@import` não enfileirados, mesmas folhas pedidas com o filtro desligado | idem |
| Cosmético: caixas escondidas e contadas uma vez, vence o `display` da página, página hostil funda | idem |
| `adblock=` nos Ajustes (padrão ligado, ida e volta, lixo ignorado), geometria do botão e do balão, glifos `Shield`/`ShieldOff` | `system::settings::tests`, `ui::layout::tests`, `ui::iconart::tests` |
| Fuzz: `fuzz/fuzz_targets/html_img_form.rs` ganhou `ImgState::Blocked` e `shields_exercise` (endereços, página, lista de sites, cosmético e folhas com bytes hostis) | ver CHANGELOG |
| QEMU (BIOS), página `/ads` de `tools/w24-site.py`, `tools/perf/scen/w53-shields.sh`: ligado, a serial diz `shields: page 203.0.113.5 on=true ads=4 trackers=2 hidden=2 total=8` e o servidor só vê `/ads` e `/g.png`; pela chave do balão o site é desligado, a página recarrega e o servidor vê as cinco requisições (folha, dois anúncios por caminho, pixel e a imagem da caixa escondida) e o guest tenta os dois hosts `.invalid`; ligado de novo, volta ao mesmo total | `docs/img/browser-shield-on.png`, `browser-shield-off.png`; 21 verificações PASS no `w53-result.txt` |
| A escolha por site sobrevive a um reinício (`~/.shields`) | `w53-persist-1.sh` e `w53-persist-2.sh` (`FS_IMG` do primeiro) |
| A chave global de Ajustes abre a página sem filtro | `w53-settings.sh` |

## 11b. CPU e congelamentos

Três causas, medidas com `perf-trace` (`path ...`, `cpu samples` por segundo) e `callgrind` sobre o exemplo `render_page`:

1. **Espera ocupada na rede.** `netstack::Net::pause` (usada por `pump` e pelo `Read`/`Write` do TLS) gira alguns polls
   e depois dorme um tick de 4 ms com `sched::block` (só em thread de trabalho com interrupções ligadas).
2. **Custo do layout.** `matched_decls` guarda, por elemento (endereço e tipo de pseudo-elemento), as regras que casam
   (`Stylesheet::memo`, esvaziado no começo e no fim de cada layout) e `css/bloom.rs` filtra os seletores com
   ancestrais pelo caminho do elemento (`Stylesheet::path_bloom`).
3. **Número de layouts.** `Relayout` (`browser/state.rs`) junta as chegadas de folhas e imagens; `Desktop::browser_flush_layout`
   (chamado por `bringup::pump_browser`) decide quando.

Reproduzir: `tools/fetch-page-css.py URL /tmp/p` e `cargo run --release -p kitsune_core --example render_page -- /tmp/p/page.html 960 100 --real-font --css /tmp/p/css-*.css`
(os tempos saem no stderr); no QEMU, `tools/perf/scen/w54-browser-idle.sh` com um build `--features perf-trace`.

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
| Busca dos externos | `web::script::ScriptSet`, `kernel/src/fetch` (`KIND_SCRIPT`), `bringup::pump_browser` | como as folhas: um por vez pelo `fetcher`, depois das folhas e antes das imagens; até 48 scripts e 10 MiB por página, cada um até 4 MiB (um script cortado ou que não descompacta é descartado; antes do §16 eram 4 MiB e 1 MiB) |
| Ids dos elementos | `Element::uid`, `number_elements` | cada elemento ganha um número em ordem de documento; é por ele que um clique acha o elemento no processo e que o documento devolvido mantém a identidade |
| Mensagens | `web::script::{init_message, click_message, parse_message}` | navegador → processo: JSON (`init` com a árvore, a URL, o idioma, o `Referer`, o viewport e quantos scripts vêm; depois um `script` por mensagem, `{"t":"script","name":...,"el":N}` + nova linha + o código cru, entregues conforme o processo os consome; `click` com o id, a posição e um número de sequência). Processo → navegador: um byte de tipo e o resto (`D` + HTML do documento com os ids em `data-kjs`, `A` + veredito de um clique, `N` + endereço) |
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
hover seguem desenhados durante um `for(;;){}`). **Carregar um script** (compilar e rodar o nível de cima) tem
**60 s** (`LOAD_MS`; §16): o QuickJS compila o arquivo inteiro de uma vez e um bundle grande leva segundos sob
emulação; o mesmo interrupt handler continua valendo, só o prazo é outro, e o resto da tarefa volta aos 3 s. Um
script que leva mais de 250 ms aparece no log (`jsrun: URL (N bytes) ran in M ms`). Memória: a do `jsrun` (96 MiB de
JS, heap de até 128 MiB, que só cresce com o uso; `proc: 'jsrun' pid N heap M MiB` no log a cada 16 MiB). No máximo 8
processos no sistema: sem vaga, a página fica sem JavaScript (o log diz).

### 12.2 O que os scripts encontram

`window`/`globalThis`/`self`, `document` (`documentElement`, `head`, `body`, `title` que lê e escreve,
`readyState`, `getElementById`, `getElementsByTagName/ClassName/Name`, `querySelector(All)`, `createElement`,
`createTextNode`, `createDocumentFragment`, `createComment`, `cookie` (o pote do navegador, §13), `write` que acrescenta ao
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
`Mozilla/5.0 (X11; Kitsune x86_64) Kitsune/<versão>`, `language`), `URL` e `URLSearchParams`, `atob`/`btoa`,
`alert`/`confirm`/`prompt` (vão para o log). `console.log` sai no log do sistema como `[jsrun:PID] ...`.

A rede (`fetch`, `XMLHttpRequest`, cookies, CORS, `TextEncoder`, `Blob`, `crypto`...) está no §13; o teclado, os campos,
o foco, o mouse, as medidas de layout, os observers, `<template>`, custom elements e shadow DOM, `localStorage`
persistente, `history.pushState`, `Intl`, Web Workers e o resto que os frameworks usam estão no §14.

Desde o §16: os elementos `<script>` ficam no documento (vazios), com `document.currentScript` enquanto cada um roda,
e um `<script>` que um script insere **roda** (`scripts.js`: o inline na hora, dentro da chamada que o inseriu; o
com `src` é buscado pelo navegador no modo `script` e roda como tarefa própria, seguido de `load` ou `error`; os de
`innerHTML` não rodam, como no padrão).

**Ainda não**: módulos (`type=module`, `import`), canvas (`getContext` dá `null`), `<link>` acrescentado por script
depois do início (as imagens novas são buscadas; folhas novas não). Os scripts do documento rodam todos depois do
documento inteiro (como `defer`), não no meio do parse; `document.write` acrescenta ao fim. O documento volta inteiro a cada mudança: barato
em páginas pequenas, caro numa SPA grande (cada envio é um parse e um layout). Os scripts só começam na aba ativa.

### 12.3 Provas

| O quê | Evidência |
|---|---|
| Parser guarda os scripts, ids, `replace_from_script` (folhas externas e `<style>` mantidos, entrada hostil), caixa por elemento, `ScriptSet`, mensagens e o clique que espera o veredito | `kitsune_core` `web::script::tests` (18 testes) |
| O DOM em JavaScript no hospedeiro | `cargo run -p kitsune_core --example script_init -- pagina.html URL nome.js=arquivo.js` gera o `init`; o `jsrun` compilado para o hospedeiro com um laço de teste (não versionado) rodou a página `/js` com cliques |
| A página `/js` de `tools/w24-site.py` no QEMU (BIOS, 256 MiB) | `tools/perf/scen/w52-browser-js.sh`: **19/19**. Script externo buscado, processo iniciado, scripts em ordem, `DOMContentLoaded`, `load`, `createElement`/`appendChild`/`insertAdjacentHTML`/seletores, `setTimeout`, `setInterval`, `requestAnimationFrame`, promise; o título da janela, o `h1`, a lista e um parágrafo mudam na tela; dois cliques no botão contam e trocam a classe (o fundo muda); o link com `preventDefault` não navega; o `javascript:` roda; o `for(;;){}` é interrompido (orçamento de 3 s por tarefa), o desktop segue desenhando e o terceiro clique funciona |

## 13. A rede dos scripts: fetch, XHR, cookies, CORS e conexões persistentes

Etapas 2 (o que faltava de rede) e 3 do item 11 do [`ROADMAP`](../ROADMAP.md). O JavaScript das páginas tem rede de
verdade: `fetch` e `XMLHttpRequest` passam pelo mesmo `fetcher` do navegador, com cookies, a regra de mesma origem
(CORS), o bloqueador e conexões HTTP/1.1 mantidas abertas com a sessão TLS.

### 13.1 Caminho de um pedido

```text
script (jsrun --page)              navegador (compositor)                fetcher (thread da rede)
fetch(url, init) --F msg (57)-->   PageShield::check  --bloqueado--> erro ao script, conta no escudo
                                   post_script_fetch  --fila (256)-->  serve.rs: CORS, pré-voo, cookies,
                                                                       redirecionamentos, Net::exchange
<--- cabeça (JSON) / pedaços (binário, até 256 KiB) / fim | erro --- page_try_post (56), direto ao processo
```

| Peça | Onde | O quê |
|---|---|---|
| Mensagens | `kitsune_core::web::script::net` | `F` (pedido: id, método, URL, `mode`, `credentials`, `redirect`, cabeçalhos, corpo binário), `X` (abortar), `C` (`document.cookie = ...`); de volta, a cabeça e o fim em JSON e os pedaços do corpo como mensagem binária (`0x01`, id, bytes) |
| HTTP/1.1 | `kitsune_core::http1` | `ResponseReader` (onde a resposta termina: `Content-Length`, `chunked` sem a moldura, até o fechamento; `1xx` pulados; `HEAD`/`204`/`304` sem corpo; keep-alive), `write_request`, `Target` (URL sem o limite de 480 bytes), `resolve` de `Location`, `Content-Range` |
| Cookies | `kitsune_core::cookies` (`Jar`), `kernel/src/fetch/cookies.rs` | ver §13.3 |
| CORS | `kitsune_core::cors` | ver §13.4 |
| Conexões | `kernel/src/netstack/{pool,http,tls}.rs` | ver §13.2 |
| Fila dos scripts | `kernel/src/fetch/script.rs`, `script/serve.rs` | um pedido por vez, em ordem, depois dos pedidos do próprio navegador (página, folhas, scripts, imagens) e do shell; abortar (`X`, a aba trocar de página ou fechar) para a transferência e fecha a conexão |
| Navegador | `kernel/src/desktop/apps/browser/jsnet.rs` | escudo, fila, `document.cookie`, `~/.cookies` |
| JavaScript | `user-apps/jsrun/src/bin/jsrun/net.js` (depois do `dom.js`), `src/page.c` | ver §13.5 |

O corpo vai **direto** do `fetcher` para a fila de mensagens do processo (`userproc::page_try_post`), sem passar pelo
compositor: um pedaço a cada 256 KiB ou 40 ms. Quando a fila do processo está cheia (8 MiB ou 256 mensagens), o
`fetcher` espera um tick e tenta de novo: é a contrapressão de um stream que ninguém lê (o log diz quanto tempo
esperou, `ms waiting for the page`). O `page.c` pega várias mensagens seguidas (até 64 ou 20 ms) antes de rodar as
tarefas.

### 13.2 Conexões persistentes (keep-alive) e TLS reutilizado

O `Net` tem **4 sockets TCP** (janela de 64 KiB cada; antes era um socket de 8 KiB), e cada um pode guardar uma
conexão aberta com a sua sessão TLS (`pool::Slot`, buffers de registro estáticos por vaga). Um pedido usa a conexão
ociosa com a mesma chave (esquema, host, porta, a exceção de certificado da pessoa, e se é de um app) se ela está
`Established`, sem nada por ler e ociosa há menos de **15 s**; senão abre outra numa vaga livre ou na ociosa mais
antiga. Fecha em erro, resposta lida até o fechamento, `Connection: close`, HTTP/1.0 sem `keep-alive`, bytes a mais
depois da resposta, ou pedido abortado. Se o servidor tinha fechado a conexão reaproveitada e nada da resposta chegou,
o pedido é refeito **uma vez** numa conexão nova. Todo pedido da página, das folhas, das imagens, dos scripts e dos
`fetch` passa por aí (`Net::exchange`), e o log conta: `net: new connection ...`, `net: reusing connection ...`,
`[net: N connections, H TLS handshakes, R reuses]`.

Leitura sem cópia a mais: o corpo decifrado é lido do próprio registro do `embedded-tls` (`read_buffered`) e
entregue ao leitor HTTP; o leitor passa fatias; para o script, uma cópia para o pedaço da mensagem e outra para a
memória do processo. O `Stream` do TLS só gira o `poll` da interface quando o socket está vazio.

Enquanto o `fetcher` está ocioso, o serviço da rede (`netd`) lê a placa direto (DHCP, ARP, ping) e os segmentos de uma
conexão ociosa se perdem; por isso o fechamento de uma conexão ociosa pelo servidor só é notado no próximo uso (e o
pedido é refeito, como acima).

### 13.3 Cookies

`kitsune_core::cookies::Jar` (RFC 6265 com as regras do 6265bis que os navegadores aplicam): `Set-Cookie` com
`Domain` (o domínio precisa ser o site do host ou abaixo dele: `Domain=co.uk` e `Domain=com` são recusados; IP só
ele mesmo), `Path` (padrão: o diretório), `Expires` (o formato de datas do §5.1.1, tolerante) e `Max-Age` (vence o
`Expires`; no máximo 400 dias), `Secure` (só de e para https; uma página http não sombreia um cookie `Secure`),
`HttpOnly` (o script não lê nem escreve), `SameSite` (`Strict` só para pedidos do mesmo site; `Lax` e sem atributo
também na navegação de topo com `GET`; `None` exige `Secure`; uma resposta de outro site só grava `None`), prefixos
`__Secure-` e `__Host-`. Envio ordenado por caminho mais longo e depois mais antigo. Limites: 4 KiB por cookie, 50
por domínio, 1000 no total (sai o mais antigo).

O `fetcher` tem um pote só (o do navegador): toda navegação manda e guarda cookies; folhas, imagens, scripts e
`fetch` são subrecursos da página da aba ativa (`set_page`) e decidem o contexto pelo site (`shields::site_of`).
**Com o escudo ligado, um subrecurso de outro site não manda nem guarda cookies** (log `cookies: third-party
cookies blocked for ...`). Apps (`net_http_get`) e o shell (`curl`) não veem o pote. `document.cookie` lê do pote
(o que não é `HttpOnly`) e escreve nele (`C`); o valor chega no `init` e em cada cabeça de resposta.

Persistência: os cookies com validade vão para `~/.cookies` (`/home/<nome>/.cookies`, texto com um cookie por linha
e campos separados por tab; uma conta logada não escreve em `/home`), no máximo a cada 3 s depois de uma mudança, e
são lidos quando uma janela do navegador abre. Os de sessão somem ao reiniciar.

### 13.4 Mesma origem (CORS) e `Origin`/`Referer`

`kitsune_core::cors`, aplicado pelo `fetcher` em cada salto:

* Mesma origem (esquema, host e porta da página): tudo é legível, inclusive cabeçalhos (menos `Set-Cookie`).
* Outra origem em `mode: 'cors'` (o padrão): leva `Origin`; a resposta só é entregue se
  `Access-Control-Allow-Origin` for `*` (sem credenciais) ou a origem da página; com `credentials: 'include'` (ou
  `withCredentials`) também `Access-Control-Allow-Credentials: true` e origem exata. Senão: `TypeError` no script e
  `cors: blocked ...` no log. O script lê só os cabeçalhos da lista segura e os de `Access-Control-Expose-Headers`.
* Método fora de `GET`/`HEAD`/`POST` ou cabeçalho fora da lista segura (`Accept`, `Accept-Language`,
  `Content-Language`, `Content-Type` simples, `Range` simples): **pré-voo `OPTIONS`** com
  `Access-Control-Request-Method`/`-Headers`, conferido (`2xx`, origem, métodos, cabeçalhos, `*` sem credenciais).
* `no-cors`: só pedidos simples; a resposta é opaca (status 0, sem cabeçalhos nem corpo). `same-origin`: outra
  origem é erro.
* Cookies seguem `credentials` (`omit`, `same-origin` padrão, `include`) além das regras do §13.3.
* `Origin` vai em pedidos a outra origem e em métodos que não são `GET`/`HEAD`; `Referer` segue
  `strict-origin-when-cross-origin` (a URL inteira para a mesma origem, só a origem para outra, nada de https para
  http). Cabeçalhos proibidos (`Cookie`, `Host`, `Origin`, `Referer`, `Content-Length`, `Sec-*`, `Proxy-*`...) que o
  script tente pôr são descartados. Uma página https não busca http (conteúdo misto).

**Simplificado:** sem cache de pré-voo (cada pedido não simples faz o seu `OPTIONS`); um redirecionamento para outra
origem mantém o `Origin` original (o padrão o troca por `null` e a resposta passa a exigir CORS); `referrerPolicy` e
`integrity` são ignorados; o contexto de cookies de uma navegação é sempre o do próprio site (não sabemos quem
iniciou a navegação, então um formulário POST de outro site leva os cookies `Lax`).

### 13.5 O que os scripts encontram

`fetch` (GET/POST/PUT/DELETE/HEAD/PATCH e métodos próprios; corpo `string`, `ArrayBuffer`/views, `Blob`, `FormData`
(multipart), `URLSearchParams`; `headers`, `mode`, `credentials`, `redirect` (`follow`, `error`, `manual` dá
`opaqueredirect`), `signal`), `Request`, `Response` (`status`, `ok`, `statusText`, `headers`, `url`, `redirected`,
`type`, `text`/`json`/`arrayBuffer`/`blob`/`bytes`/`formData`, `clone`, `Response.json/error/redirect`) com **`body`
como `ReadableStream`** (`getReader().read()` em pedaços, `for await`, `tee`, `cancel`), `Headers`,
`AbortController`/`AbortSignal` (`abort`, `timeout`, `any`), `XMLHttpRequest` (assíncrono; `responseType` texto, `json`,
`arraybuffer`, `blob`; eventos `readystatechange`/`loadstart`/`progress`/`load`/`error`/`abort`/`timeout`/`loadend`;
`timeout`; `withCredentials`; cabeçalhos da resposta), `navigator.sendBeacon`, `Blob`, `File`, `FormData`,
`TextEncoder`/`TextDecoder` (UTF-8 com `stream` e `fatal`, latin1, UTF-16LE), `crypto.getRandomValues` e
`crypto.randomUUID` (do gerador do kernel pela **chamada 66 `getrandom`**), `structuredClone`, `URL.createObjectURL`
(`blob:`) e `data:` no `fetch`, `URL.canParse`, `performance.timeOrigin/mark/measure`; `URLSearchParams` com `+` para
espaço, `size` e `sort`. `atob`/`btoa`, `queueMicrotask` e `performance.now` já existiam.

Limites: `XMLHttpRequest` síncrono roda assíncrono (avisa no console); corpo de pedido em stream não; resposta até 64
MiB (o processo tem 96 MiB de JS para tudo); `gzip`/`deflate` não é pedido (`Accept-Encoding: identity`), e se vier é
descompactado inteiro no fim; `crypto.subtle` não existe; **`WebSocket` não foi feito** (pendente: `ws://` e `wss://`
sobre a mesma pilha, com o `Upgrade` e o enquadramento no `fetcher`). Um download grande de um script ocupa o
`fetcher` até acabar (folhas e imagens esperam).

### 13.6 Medidas (QEMU TCG, sem KVM)

Página `/net/speed` (dois downloads de 8 MiB lidos com `getReader`, na mesma conexão), servidor local
(`tools/w24-site.py --tls`), cenário `w55-net-js.sh` com `SPEED_HTTP=1`:

| Placa | http | https (TLS 1.3, AES-128-GCM em software) | handshake |
|---|---|---|---|
| `virtio-net-pci` | 26,0 e 29,0 MB/s | 8,2 e 6,7 MB/s | 3,9 s |
| `ne2k_isa` (padrão do `run.sh`) | 7,3 e 6,2 MB/s | 3,2 e 4,8 MB/s | 4,2 s |

Com a virtio a diferença http/https é a cifra: AES e GHASH em software (o kernel é compilado sem SSE, então nem
AES-NI nem os caminhos SIMD do RustCrypto servem; ver `.cargo/config.toml`). Ficou ~8 MB/s, o suficiente para um
segmento de vídeo de 10 MiB em pouco mais de 1 s; na NE2000 a placa (PIO) divide a CPU com a cifra.

Um achado da medida: com a janela TCP de 64 KiB a NE2000 perdia quadros (o anel de recepção dela guarda ~14 KiB e a
placa é lida por polling), cada perda custava um tempo de retransmissão e o **primeiro** download grande de uma conexão
saía a 0,9 MB/s (4 MiB em 4,7 s; o seguinte, já com a janela de congestionamento do servidor reduzida, a 7 MB/s). Com
a NE2000 a janela agora é de 10 KiB (`NE2000_WINDOW`): o mesmo stream de 4 MiB passou a 7,0 MB/s.

Handshakes: na página `/net` (23 pedidos do script, mais a página e o script) o servidor viu **3 conexões** (uma para a
página e outra para a segunda origem, e uma refeita depois do pedido abortado); na `/net/speed` em https, **1
handshake** para a página e os dois downloads (antes seriam 3, ~4 s cada no TCG).

Uma página real pela rede de verdade (`w50-real-site.sh`, `https://en.wikipedia.org/wiki/Kitsune`, virtio-net, com
"continuar mesmo assim" porque o proxy deste ambiente reassina o HTTPS): 7 pedidos ao host da página (a página, duas
folhas, o script de inicialização e três imagens) em **4 handshakes** (antes, 7). As conexões novas vieram depois de
pausas maiores que os 15 s de ociosidade (o layout da página leva segundos no TCG); as imagens de outros hosts
(`upload.wikimedia.org`) falham no certificado reassinado, como antes.

### 13.7 Provas

| O quê | Evidência |
|---|---|
| Leitor HTTP/1.1 (cortes em qualquer byte, chunked, 1xx, HEAD/204/304, Content-Length conflitante, cabeçalho grande, 3 MiB em fatias), `Target`, `resolve`, `write_request`, `Content-Range` | `kitsune_core` `browsing::http1::tests` |
| Cookies (Domain/Path/Expires/Max-Age/Secure/HttpOnly/SameSite, prefixos, limites, datas, persistência e arquivo danificado) | `browsing::cookies::tests` |
| CORS (pré-voo, lista segura, Allow-Origin/Credentials, cabeçalhos expostos, Referer, proibidos) | `browsing::cors::tests` |
| Mensagens `F`/`X`/`C` e as de volta | `web::script::net::tests` |
| `fetch`/XHR/streams/abort/`FormData`/`TextDecoder` em stream no JavaScript, com um navegador falso | `node tools/js-net-check.js user-apps/jsrun/src/bin/jsrun` |
| Tudo junto no QEMU (BIOS, 256 MiB), página `/net` de `tools/w24-site.py` (rotas em `tools/w24net.py`) | `tools/perf/scen/w55-net-js.sh`: **23/23** verificações do script e 8 do log (cookies guardados e salvos em `~/.cookies`, CORS recusado e pré-voo no log, o escudo contou o `fetch`, conexões reutilizadas), repetido três vezes; `docs/img/browser-net-js.png` |
| Nada quebrou com o keep-alive no servidor de teste e no navegador | `w52-browser-js.sh` 19/19 e `w53-shields.sh` 21/21 com o mesmo build |
| Página real (Wikipédia) com cookies e conexões reutilizadas; um pânico do CSS achado e corrigido | `w50-real-site.sh` (§13.6) |

## 14. O DOM para frameworks: entrada, medidas, observers, estrutura, armazenamento e histórico

Resto da etapa 2 do item 11 do [`ROADMAP`](../ROADMAP.md) (aceite: um app pequeno de Preact e um de Vue funcionam de
verdade) e preparo da etapa 6. O código novo do processo está em arquivos próprios, carregados depois do `dom.js` e do
`net.js` na ordem de `jsrun.rs` (cada um constrói sobre os anteriores pelo objeto interno `kjs.dom`, que o `dom.js`
exporta); o `dom.js` só ganhou ganchos: `mutators` (toda mudança da árvore), `hooks.wireAttrs`/`wireKids` (o que a
cópia do navegador leva de um elemento) e `hooks.created` (classe do elemento, custom elements).

| Arquivo | O quê |
|---|---|
| `forms.js` | o estado dos controles (`value`, `checked`, `selectedIndex`, `defaultValue`, `type`, `form`, `form.elements`, `labels`, validação, seleção do texto) e o que a cópia do navegador leva deles |
| `events.js` | classes de evento (`KeyboardEvent`, `MouseEvent`, `PointerEvent`, `WheelEvent`, `InputEvent`, `FocusEvent`, `SubmitEvent`, `PopStateEvent`...), `addEventListener` com `once`/`passive`/`signal`, foco (`focus()`/`blur()`, `activeElement`, `autofocus`), ações padrão de um clique (caixa e rádio, `label`, `summary`, link, envio com `submit`), `form.submit()`/`requestSubmit()`/`reset()` e as mensagens de entrada |
| `layoutapi.js` | o retrato do layout: `getBoundingClientRect`, `offset*`, `client*`, `scroll*`, `scrollTo`/`scrollBy`/`scrollIntoView`, `innerWidth`/`scrollY`/`devicePixelRatio`, `visualViewport`, `elementFromPoint`, `getComputedStyle`, `matchMedia` (com `change`), `CSS.supports`, `DOMRect` |
| `observers.js` | `MutationObserver` de verdade, `IntersectionObserver`, `ResizeObserver`, `requestIdleCallback`, `PerformanceObserver` |
| `structure.js` | classes dos elementos (`HTMLInputElement`...; `instanceof` funciona), `<template>`/`content`, `style` como `CSSStyleDeclaration`, `TreeWalker`/`NodeIterator`, `Range`, `Selection`, `compareDocumentPosition`, `normalize`, `splitText`, custom elements e shadow DOM |
| `storage.js` | `localStorage` (persistente, por origem) e `sessionStorage` |
| `history.js` | `history.pushState`/`replaceState`/`back`/`forward`/`go`/`state`/`length`, `popstate`, `hashchange`, `location` (`hash`, `search`, `pathname`... que navegam) |
| `intl.js` | um `Intl` pequeno (pt-BR e en): `DateTimeFormat`, `NumberFormat`, `PluralRules`, `Collator`, `RelativeTimeFormat`, `ListFormat`, `Segmenter`, e os `toLocale*String`/`localeCompare` |
| `webapis.js` | `window.open` (nova aba), `postMessage` na mesma janela, `MessageChannel`, `BroadcastChannel` (só a página), `a.pathname` e afins, `href`/`src` absolutos, `navigator` (`clipboard`, `permissions`, `serviceWorker`, `storage`...), e os que recusam sem quebrar: `Notification` (negada), service workers (rejeitados), `WebSocket` (falha como servidor inalcançável) |
| `scripts.js` | `<script>` inserido por script (inline na hora, `src` pelo navegador no modo `script`), `load`/`error`, propriedades de `HTMLScriptElement` (§16) |
| `compat.js` (o último) | `Image`, `Option`, `canvas.getContext` (`null`), `mediaCapabilities`, `mediaSession`, `CDATASection`, `ProcessingInstruction` (§16) |
| `msgcodec.js`, `workers.js` | o clone estruturado em texto (`Date`, `RegExp`, `Map`, `Set`, `ArrayBuffer`, arrays tipados, `Blob`/`File`, `Error`, referências repetidas) e `Worker` do lado da página (§14.5) |
| `workerscope.js` | o escopo global do worker (prelúdio do `jsrun --worker`: `dom.js` só pelos timers e pelo laço, `net.js`, `intl.js`, `msgcodec.js` e ele) |

### 14.1 Entrada: quem manda nos controles

O navegador continua desenhando e editando os campos (cursor, texto, caixa marcada) sem esperar o processo: digitar
não fica preso à velocidade do JavaScript. A página recebe cada tecla **com o texto do campo depois dela** e o estado
dos controles passa a viver no processo (`value`, `checked`, `selectedIndex`); a cópia do documento que volta ao
navegador leva esse estado (o `value` atual no atributo `value`, `checked`, a `<option>` escolhida, o texto do
`<textarea>`), e o navegador refaz o seu `FormState` a partir dela a cada documento (`script::relayout_js`): o que um
script atribui (`input.value = ''` depois do Enter) aparece na tela, e o foco e o cursor ficam no mesmo controle
(identificado pelo `uid` do elemento, `FieldInfo::uid`).

A corrida entre o que se digita e o documento que o processo manda é resolvida com uma numeração: toda entrada
enviada leva `n`; o processo responde `Q<n>` antes do documento que já reflete as entradas até `n`. O navegador
guarda quais controles tiveram o texto mudado por teclas ainda não reconhecidas (`Inputs::edited`/`unseen_edits`);
se chega um documento que ainda não as viu, esses controles mantêm o texto do navegador (o próximo documento o
trará). Sem isso, digitar rápido perdia letras. Uma mudança feita pelo navegador sem tecla (o `<select>` avançado
por um clique) faz a página mandar o documento de novo, para a cópia do navegador seguir.

| Mensagem | Quando | Na página |
|---|---|---|
| `key` (`k`, `c`, `kc`, `m`, `uid`, `v`) | cada tecla num campo focado; na página sem campo focado só se ela escuta teclas (`W key`) | `keydown`, `keypress`, `input` (com `inputType`), `keyup`; `change` no Enter ou ao sair do campo; `preventDefault` no `keydown` desfaz a mudança |
| `key` com `seq` | Enter num campo de um `<form>` de verdade | `submit` (cancelável); o envio espera o veredito `A seq 0/1` como o clique (§12.1). Controles fora de formulário nunca são enviados |
| `focus` (`uid`) | o foco do navegador mudou (clique, Tab, Esc) | `blur`/`focusout`, `focus`/`focusin`, `activeElement` |
| `G<uid>` (da página) | `el.focus()` / `blur()` num controle | o navegador põe o cursor lá |
| `field` (`ck`, `si`, `v`) | um clique avançou um `<select>`, o Espaço marcou uma caixa | `input`/`change` (a caixa vira o `click` que representa) |
| `click` (`uid`, `x`, `y`, `cx`, `cy`, `seq`) | clique | `pointerdown`, `mousedown`, foco, `pointerup`, `mouseup`, `click` com as ações padrão: caixa/rádio (desfeita se `preventDefault`), `label` → controle, `summary` → `details`, link `#fragmento` da própria página (entrada do histórico, sem recarregar), botão de envio → `submit` |
| `mouse` (`move`, `dbl`, `ctx`, `wheel`) | `move` só se a página escuta movimento (`W move`), no máximo um por quadro; os outros só se escuta (`dblclick`, `contextmenu`, `wheel`) | `mousemove`/`pointermove`, `mouseover`/`out`/`enter`/`leave` calculados no processo |
| `W<tipos>` (da página) | um `addEventListener`/`onX`/atributo `onX` de um tipo pedido sob demanda (`move`, `key`, `wheel`, `scroll`, `ctx`, `dbl`) | a lista cresce, nunca diminui |
| `vis` | a aba aparece ou some | `visibilitychange`, `document.hidden`, `pageshow`/`pagehide` |

O `contextmenu` não impede o menu do navegador e a roda não é cancelável (o navegador já rolou).

### 14.2 Medidas: o retrato do layout

O layout já registra a caixa de cada elemento quando a página tem scripts (`Doc::set_track`); agora registra também
um resumo do estilo calculado de cada elemento (`Page::styles`, `StyleRec`: `display`, cor, fundo, tamanho e peso da
fonte, posição, margens, enchimentos, bordas). **Decisão: o retrato vai ao processo depois de cada layout**, numa
mensagem binária só (`script::boxes_message`, dado de rede com id 0): 52 bytes por elemento (uid, a união das caixas
do elemento em px CSS, coordenadas da página, e o estilo), mais a viewport e a rolagem. Pedir sob demanda exigiria uma
ida e volta síncrona ao compositor por medida (um quadro cada) e os frameworks medem em laços; o custo de mandar
sempre é pequeno perto do próprio layout (uma página de 2000 elementos: ~100 KiB por layout). O processo lê tudo
sincronamente desse retrato. **Limite**: um elemento criado depois do último layout tem caixa zero até o layout do
documento que o contém (no mesmo quadro na prática: o processo manda o documento, o navegador faz o layout e manda o
retrato); `getComputedStyle` cobre as propriedades de caixa e cor mais usadas (o resto vem do estilo em linha ou de
valores iniciais), sem pseudoelementos. `view` leva a rolagem e o tamanho quando mudam sem layout (a cada quadro de
rolagem se a página escuta `scroll` ou tem um `IntersectionObserver`, senão quando a rolagem para) e dispara `scroll`
e `resize`. `scrollTo`/`scrollIntoView` mandam `S<x> <y>` e o navegador rola.

Os observers seguem o retrato: `ResizeObserver` compara o tamanho do conteúdo de cada alvo depois de cada retrato,
`IntersectionObserver` calcula a interseção com a viewport (ou o `root`), com `rootMargin` e `threshold`, depois de
cada retrato e cada rolagem; ambos entregam numa tarefa. `MutationObserver` usa o gancho `mutators` do `dom.js`
(`childList`, `attributes` com `attributeFilter`/`oldValue`, `characterData`, `subtree`) e entrega num microtask.

### 14.3 Estrutura, custom elements e shadow DOM

`<template>` guarda o conteúdo num `DocumentFragment` (`content`); `innerHTML` do template escreve nele. Os elementos
ganham a classe certa (`HTMLInputElement`, `HTMLButtonElement`...), menos `<video>`, `<audio>`, `<source>` e `<track>`
(do código de mídia). `customElements.define/get/whenDefined/upgrade/getName`: elementos já no documento são
atualizados na definição, `connectedCallback`/`disconnectedCallback`/`attributeChangedCallback` (com
`observedAttributes`) rodam nas mudanças da árvore. `attachShadow({mode})` cria um `ShadowRoot`; os eventos de dentro
chegam ao host e acima (sem retarget: quem escuta fora vê o alvo de dentro), `getRootNode`, `isConnected`,
`assignedSlot`, `slot.assignedNodes()`. **O navegador desenha a árvore sombra no lugar dos filhos do host** (gancho
`wireKids`), com cada `<slot>` trocado pelos filhos do host atribuídos a ele (por `slot="nome"`) ou pelo seu conteúdo
padrão; um `<style>` dentro da sombra vale para a página toda (sem escopo, sem `:host`, sem `::slotted`, sem
`adoptedStyleSheets`, que não anunciamos para que bibliotecas como o Lit usem `<style>`). `<iframe>` não tem layout
aninhado: o elemento existe no DOM, sem documento dentro.

### 14.4 `localStorage` e histórico

`localStorage` fica no navegador por origem (`kitsune_core::web::script::WebStorage`), no arquivo `~/.webstorage`
(uma linha por item, `origem TAB chave TAB valor`, com escapes), lido ao abrir a janela do navegador e escrito no
máximo a cada 3 s depois de uma mudança (`webstorage: saved` no log). A página recebe os itens da sua origem logo
depois do `init` (`store`) e manda cada mudança (`L` + `["set", k, v]`, `["del", k]`, `["clear"]`). Cotas: 512 Ki
unidades UTF-16 por origem no processo (`QuotaExceededError`), 1 MiB por origem e 4 MiB no todo no navegador, 64
origens (a menos usada sai). `sessionStorage` vive com o processo (uma recarga começa vazia). O evento `storage` entre
abas não existe ainda.

`history.pushState`/`replaceState` mandam `H push|replace <url>`; o navegador aceita só a mesma origem
(`Browser::push_state`), troca o endereço da barra (a menos que a pessoa esteja editando) e grava a entrada com o
número do documento da entrada atual (`History::docs`). Voltar ou avançar para uma entrada do mesmo documento não
carrega nada: o navegador manda `pop` (`url`, índice) e a página dispara `popstate` (e `hashchange` se só o fragmento
mudou). Uma recarga, ou chegar a uma entrada de outro documento, é um documento novo. `location.hash = ...` e um
clique num link `#fragmento` da própria página viram entradas do mesmo documento (com `hashchange` e a rolagem até o
alvo) em vez de recarregar a página: é o roteamento de muitos apps (o TodoMVC). `history.go(n)` manda `B<n>`.
`beforeunload` nunca dispara (o navegador não pergunta antes de sair).

### 14.5 Web Workers

`new Worker(url)` (clássico; `type: 'module'` recusa) roda num **processo próprio**: outro `jsrun`, iniciado pelo
navegador com `--worker` (`userproc::spawn_worker`, a mesma ligação de mensagens da página, chamadas 56/57; nenhuma
chamada nova). A página busca o script com o seu `fetch` (mesma origem, `blob:` ou `data:`; outra origem é
`SecurityError`) e manda `K new` com o código; o navegador (`kernel/.../browser/jsworker.rs`) inicia o processo e lhe
entrega `winit`. `postMessage` vai e volta como texto (`msgcodec.js`), carregado pelo navegador sem ler (`K msg` →
`wmsg`; `M` → `worker`). O worker tem `self`, `postMessage`, `onmessage`, `close()`, `name`, `location` (só leitura),
timers, `requestAnimationFrame`, `fetch`/XHR (pelo mesmo bloqueador e fetcher da página, as respostas vão direto ao
processo do worker), `Intl`, `TextEncoder`, `crypto`, e não tem `document` nem `window`. **No máximo 2 por página**
(`MAX_WORKERS`; o terceiro recebe `error` e não começa: o sistema tem 8 processos de anel 3 ao todo). `terminate()`
mata o processo; `close()` pede ao navegador que o termine; a página que sai leva os seus workers junto. Limites:
`importScripts` recusa (`NotSupportedError`; buscar síncrono exigiria uma espera bloqueante no meio de uma tarefa),
sem transferência de posse (`transfer` copia), sem `SharedWorker`, um erro não capturado num `onmessage` do worker
chega à página como `error`, um erro numa tarefa qualquer só no log.

### 14.6 Limites

`WebSocket` só recusa (ainda não há conexão persistente bidirecional na pilha para scripts), `Notification` e service
workers recusam, `navigator.clipboard` recusa (a área de transferência do sistema não é dada à página),
`BroadcastChannel` e `postMessage` só dentro da página. `Intl` cobre pt-BR e inglês; fusos: UTC e os fixos do Brasil.
Sem `contenteditable` editável, sem seleção de texto da página visível ao script (`getSelection` é um objeto que o
script pode preencher), sem `:hover`/`:focus` nos seletores do script. O documento continua indo inteiro ao
navegador a cada lote de mudanças (§12.2). Os botões continuam sendo desenhados como controles do navegador (sem o
fundo e a borda do CSS da página). `mousedown`/`mouseup` chegam juntos com o `click` (o navegador manda o clique ao
apertar o botão: não há arrastar com o mouse para a página), e só o botão esquerdo clica (o direito é o
`contextmenu`). `document.referrer` é o `Referer` da navegação (um link, formulário ou `location.href` de outra
página; vazio para um endereço digitado; §16).

### 14.7 Provas

| O quê | Evidência |
|---|---|
| Mensagens de entrada, `Subs`, `Inputs`/`Q`, `view`, o retrato do layout (caixa e estilo por elemento, `display:none`), `WebStorage` (cotas, arquivo, origens), o parser das operações `L` (escapes, surrogates, entrada hostil), `FieldInfo::uid`/`field_by_uid`, `FormState::set_text` | `kitsune_core` `web::script::tests_input` (8 testes) |
| Histórico do mesmo documento: `push_state`, `replace`, `go`, `take_pop`, outra origem recusada, recarga é documento novo | `browser::ui_tests::same_document` (4 testes) |
| Nomes de atributo dos templates (`@click`, `:class`, `v-model.trim`) | `web::dom::html_tests` |
| Mensagens dos workers | `web::script::tests_input::worker_messages` |
| O DOM inteiro em JavaScript no hospedeiro, com o navegador simulado (inclusive com a árvore do parser do navegador: `KITSUNE_INIT=script_init`): 34 verificações de API (MutationObserver, template, classes, formulários, valores no documento, custom elements com slots, shadow DOM, retrato, `getComputedStyle`, `matchMedia`, observers, `once`/`signal`, mouse, teclas, `localStorage`, `pushState`, `Intl`, `Range`, `TreeWalker`, `postMessage`, `window.open`), o TodoMVC de Preact + htm (24), o app Vue 3 (15) e os workers num segundo contexto (6: escopo sem `document`, clone estruturado nos dois sentidos, `fetch` no worker, limite de 2, `close()`, outra origem) | `node tools/js-dom-check.js user-apps/jsrun/src/bin/jsrun tools/site-assets` |
| **Preact, Vue e um Worker no QEMU** (BIOS, 256 MiB, TCG, sem KVM), digitando e clicando de verdade pelo monitor nas páginas `/preact`, `/vue` e `/worker` de `tools/w24-site.py` | `tools/perf/scen/w59-spa.sh`: **24/24**. Preact + htm: carrega, `input.focus()` num efeito põe o cursor no campo, `getBoundingClientRect` depois do layout (num `ResizeObserver`), o texto digitado chega (`input`, `value`), Enter (`keydown`) acrescenta e o campo é limpo (`value = ''` aparece na tela), três itens, a caixa marca um (`click`, `change`), o link `#/active` filtra (`hashchange`) e a barra mostra `.../preact#/active` (`pushState`), remover, `localStorage` gravado em `~/.webstorage` e a lista de volta depois de Ctrl+R. Vue 3.5 (template no DOM compilado pelo `vue.global.prod.js`): monta, `@click` conta, `v-model` + `v-if` com o nome digitado, `@keyup.enter` + `v-for`, caixa com `v-model`, `<select>` com `v-model`, formulário com `@submit.prevent` (não navega), nenhum erro nem aviso do Vue. Worker: processo próprio, conta 2262 primos até 20000, busca um arquivo e responde (~0,7 s no TCG). Capturas: `docs/img/browser-preact-todomvc.png`, `docs/img/browser-vue.png` |
| Nada quebrou | com o build final: `w52-browser-js.sh` 19/19 e `w53-shields.sh` 21/21; `w55-net-js.sh` 31/31 (23 do script e 8 do log) com o build dos workers (`f5faa4d`, antes só da separação do `forms.js` e do texto mantido por controle) |

## 15. Vídeo e áudio nas páginas: `<video>`, `<audio>` e Media Source Extensions

Etapa 4 do item 11 do [`ROADMAP`](../ROADMAP.md) (o player): o que liga o JavaScript das páginas (§12–13), os
decodificadores de anel 3 ([`midia.md`](midia.md)) e o mixer do kernel ([`audio.md`](audio.md)). Um WebM (VP9 + Opus)
toca na página, com som e sincronia, por `src` ou por Media Source Extensions (o que o YouTube usa).

### 15.1 Desenho

```text
página (jsrun --page, media.js)           navegador (compositor, kernel)                 decodificador (mediadec --serve)
HTMLMediaElement, MediaSource, ─M/m──►    MediaPlayer por elemento:          ──proto──►   track buffers por fonte,
SourceBuffer, TimeRanges        ◄─JSON─   Player (estado, eventos, relógio)  ◄─────────   VP9 + Opus, decodifica à frente
                                          fluxo do mixer (crate::audio)                   dentro do orçamento
                                          quadro na tela (Cmd::Video)        fetcher ───►  (src por endereço: direto aqui)
```

| Peça | Onde | O quê |
|---|---|---|
| Lógica pura (testada no host) | `kitsune_core::media` (`browsing/media.rs` + `media/`) | `ranges` (TimeRanges), `track` (track buffer: intervalos, `remove`, despejo, de onde decodificar), `proto` (navegador ↔ decodificador), `clock` (relógio e o `AudioSink`), `player` (o player e a máquina de estados com os eventos), `wire` (página ↔ navegador), `controls` (geometria dos controles) |
| Layout | `web::layout` (`flow.rs`, `objects.rs`, `table/measure.rs`) | `<video>` e `<audio controls>` são objetos substituídos: caixa pelo `width`/`height` (atributo ou CSS), a outra medida pela proporção do vídeo (conhecida quando os metadados chegam: `Doc::set_media_size`) ou do pôster, 300x150 sem nada; `<audio controls>` 300x40; os filhos (`<source>`, texto de fallback) não aparecem. `Cmd::Video { uid, controls, audio, poster }` e `Page::videos` |
| Pintor | `kernel/src/desktop/apps/browser/media/paint.rs` | o quadro atual (ou o pôster antes de tocar) em `object-fit: contain`, fundo preto, e os controles: play/pausa, tempo, barra de tempo com o botão, mudo, volume, e um play grande no meio do vídeo parado |
| Players | `kernel/src/desktop/apps/browser/media.rs` + `media/` | um `MediaPlayer` por elemento que toca (no máximo 2 por página): processo `mediadec` (`userproc::spawn_media`), fluxo do mixer (dono: o pid do decodificador, então fecha com ele), o `Player`, o download de um `src` (`direct.rs`), os cliques nos controles (`controls.rs`), as linhas do log (`stats.rs`) |
| JavaScript | `user-apps/jsrun/src/bin/jsrun/media.js` (depois do `net.js`) | ver §14.3; o `dom.js` só ganhou o registro de classes por tag (`elementClasses`) |
| Mensagens | `web::script::parse_message` (`V`, `v`), `media::wire` | página → navegador: `V` + `op id args` (`open`, `close`, `src`, `play`, `pause`, `seek`, `vol`, `loop`, `eos`, `dur`, `rm`, `reset`, `tso`, `cue`) e `v` + `id fonte seq\n` + bytes; navegador → página: JSON `{"t":"media","id":...}` com o estado e os eventos, o que cada fonte guarda (`buf`), erros (`fail`) |

Uma página com `<video>`/`<audio>` ganha o processo de script mesmo sem scripts (é ele que tem os elementos):
`ScriptSet::ready` olha `Doc::has_media`.

### 15.2 Reprodução e sincronia

* **O som é o relógio.** Cada player abre um fluxo no mixer (48 kHz estéreo, `crate::audio` direto, sem chamada de
  sistema) e escreve o PCM que o decodificador manda, em ordem de tempo; um buraco no som vira silêncio e uma
  sobreposição é cortada, então a posição de cada amostra no fluxo é o seu tempo. O tempo da mídia é o tempo da
  primeira amostra escrita depois do último seek mais o contador "tocado" do fluxo (o que **saiu** do dispositivo, não o
  que entrou no DMA). Pausar o player pausa o fluxo; o mudo zera o volume e o relógio segue.
* **Os quadros seguem o relógio.** A cada volta do compositor (até 250 Hz) o player mostra o último quadro com
  `pts ≤ relógio` e descarta os anteriores (atrasados); sem quadro vencido, o da tela fica. Depois de um seek o
  primeiro quadro da posição nova vai para a tela na hora (mesmo pausado).
* **Sem placa de som** (`audio: no device`), ou num vídeo sem trilha de som, o relógio do sistema corre enquanto o
  player toca, e para se as imagens acabam (com 1 s de tolerância para um decodificador lento).
* **Esperar e retomar.** Tocando, se o som acaba sem o fim do stream (ou, sem som, as imagens), o player vai a
  `waiting` (`readyState` 2, o fluxo pausado) e volta a `playing` com 300 ms de som à frente (ou o orçamento cheio, ou o
  fim). Uma margem menor que "suficiente" mas sem secar não derruba o `readyState` (sem `canplay`/`playing` repetidos).
* **Orçamento e memória.** O decodificador manda no máximo 8 MiB de quadros e 512 KiB de som sem confirmação
  (`IN_FLOW`/`IN_ACK`): a memória do kernel por player fica limitada (o heap do kernel é de 64 MiB). Os quadros vêm
  já no tamanho da caixa (o decodificador escala) e em B, G, R, A, copiados linha a linha. Os dados codificados
  ficam no decodificador (heap de até 128 MiB): o que já passou há mais de 10 s é esquecido acima de 24 MiB, e mais de
  48 MiB é recusado (`QuotaExceededError`).
* **Seek, fim, loop.** `currentTime = t`, um clique na barra, `play()` depois do fim ou o `loop` mudam a época: o fluxo
  é esvaziado, o decodificador recomeça do keyframe anterior a `t` (dentro do que está guardado; o áudio com 80 ms de
  pré-rolagem) e só a saída marcada com a época nova vale. O fim (`ended`, e antes `pause`) chega quando o stream
  terminou (`endOfStream`, ou o `src` baixado inteiro), o decodificador mandou tudo e o som tocou até a última amostra.
* **Aba em segundo plano pausa** os seus players; outra página, a aba fechada ou o processo da página morto encerram
  os players e os decodificadores.
* **Só o vídeo é repintado.** Um quadro novo marca só a caixa do vídeo como suja (`mark_dirty`; o compositor trata essa
  região sozinha como trabalho do quadro), e o pintor da página pula o resto quando a região está dentro de um vídeo.

### 15.3 O que os scripts encontram

`HTMLMediaElement`, `HTMLVideoElement`, `HTMLAudioElement`, `Audio()`: `src`, `currentSrc`, `srcObject` (só guarda),
`<source>` escolhido por `type` (`canPlayType`), `load()`, `play()` (Promise: resolve no `playing`, rejeita com
`AbortError` num `pause()`/`load()` antes, `NotSupportedError` sem fonte que toque), `pause()`, `paused`, `ended`,
`seeking`, `currentTime` (lê e escreve; antes dos metadados guarda a posição de início; entre duas mensagens do
navegador corre sozinho, sem voltar), `fastSeek`, `duration` (`NaN` sem metadados, `Infinity` desconhecida; com MSE, a
do `MediaSource`), `volume` (`IndexSizeError` fora de 0–1), `muted`, `defaultMuted`, `playbackRate`/
`defaultPlaybackRate` (guardados; só 1× toca), `readyState`, `networkState`, `buffered`, `seekable`, `played`
(`TimeRanges` com `IndexSizeError`), `videoWidth`/`videoHeight`, `error` (`MediaError` com os códigos 1–4),
`autoplay`, `loop`, `controls`, `preload`, `poster`, `playsInline`, `crossOrigin`, `width`/`height`,
`getVideoPlaybackQuality`, `textTracks`/`audioTracks`/`videoTracks` (vazios), as constantes `NETWORK_*`/`HAVE_*`, e
os eventos `loadstart`, `progress`, `suspend`, `abort`, `error`, `emptied`, `loadedmetadata`, `loadeddata`,
`canplay`, `canplaythrough`, `playing`, `waiting`, `seeking`, `seeked`, `ended`, `durationchange`, `timeupdate`
(a cada 250 ms tocando), `play`, `pause`, `ratechange`, `resize`, `volumechange` (com `on<evento>`).
`canPlayType('video/webm; codecs="vp9, opus"')` = `probably` (VP9 perfil 0 só: `vp09.00.*.08`), `video/webm` = `maybe`,
o resto `''`.

**Media Source Extensions:** `MediaSource` (`readyState` `closed`/`open`/`ended`, `duration`, `addSourceBuffer`
(até 4), `removeSourceBuffer`, `endOfStream()` (a duração passa a ser o fim do que está guardado) e
`endOfStream('decode'|'network')`, `sourceBuffers`/`activeSourceBuffers`, `setLiveSeekableRange`,
`isTypeSupported` (só WebM VP9/Opus; com `width`/`height`/`framerate`, como o YouTube sonda, até 854x480 a 30 fps: §16), `sourceopen`/`sourceended`/`sourceclose`), `SourceBuffer` (`appendBuffer`
assíncrono com `updating`, `updatestart`/`update`/`updateend`/`error`/`abort`, em pedaços de 1 MiB, cada um
confirmado pelo decodificador com o que a fonte guarda; `appendBufferAsync`; `buffered` por fonte (o do elemento é a
interseção); `remove`; `abort`; `timestampOffset`; `mode` (`segments`; `sequence` aceito sem efeito);
`appendWindowStart/End` (guardados); `changeType`), `SourceBufferList`, `URL.createObjectURL(mediaSource)` →
`blob:` que o `src` aceita. Um segundo segmento de inicialização num `SourceBuffer` recomeça o demuxer da fonte (troca
de qualidade); um append quebrado dispara `error` no buffer e `MEDIA_ERR_DECODE` no elemento.

**`src` por endereço** (`http`/`https`): o navegador baixa o arquivo direto no decodificador (`Range` de 1 MiB, no
máximo 30 s à frente da posição, pela fila de pedidos de scripts do `fetcher`, com cookies, o bloqueador e as regras
de conteúdo misto), sem CORS (como em todo navegador) e sem que o script veja os bytes. Um servidor sem `Range` manda
o arquivo inteiro de uma vez. `blob:` (de um `Blob`) e `data:` são lidos pelo script e apendados como no MSE.

**Ainda não:** velocidade diferente de 1×, legendas (`<track>`, `TextTrack`), `srcObject` com `MediaStream`,
tela cheia, Picture-in-Picture, `requestVideoFrameCallback`, `captureStream`, WebAudio (`AudioContext`,
`decodeAudioData`), EME/DRM (`requestMediaKeySystemAccess`: não existe, o que faz o YouTube servir os formatos sem
DRM), H.264/AAC/AV1/MP4 (`canPlayType` diz `''`), seek num `src` por endereço para além do que já foi baixado (o
download segue em ordem até lá), mais de 2 players por página, e o `sequence` do MSE.

### 15.4 Provas

| O quê | Evidência |
|---|---|
| TimeRanges, track buffer (duração dos quadros, regravar o mesmo tempo, `remove` com dependentes, despejo, seek ao keyframe, cursor que pula buracos, duração de pacotes Opus), relógio, player (ordem dos eventos, sincronia pelo som com atraso do dispositivo, pausa, seek com época e `MARK`, `waiting`/`playing`, fim e `loop`, buracos e sobreposições do som, sem placa, volume e mudo, quadros vencidos em espera, sem oscilar), mensagens (página ↔ navegador e navegador ↔ decodificador, hostis recusadas), controles | `cargo test -p kitsune_core media` (41 testes) |
| Layout de `<video>`/`<audio>` (300x150, tamanho do vídeo depois dos metadados, atributos e CSS, zoom, `display:block`, `<audio>` sem controles, pôster como imagem, fallback escondido, dentro de célula de tabela) e o processo de script para página só com mídia | `web::layout_tests::media` (6), `web::script::tests` |
| O decodificador: arquivo inteiro, seek, orçamento, o `src` como o `fetcher` entrega, MSE com vídeo e som em fontes separadas e o init de novo | `tools/media/mediacheck V.webm --serve-test` e `--serve-mse VIDEO AUDIO` (§ [`midia.md`](midia.md)) |
| A API em JavaScript com um navegador falso (tipos, `<source>`, `src` pelo navegador, `play()`/`pause()` e as promises, `currentTime`, seek, volume, eventos, `played`, MSE: `sourceopen`, dois buffers, pedaços de 1 MiB confirmados, `updating`, init de novo, `remove`, append quebrado, `endOfStream`, reabrir, `sourceclose`, `Blob` como `src`, `Audio()`, sem player livre) | `node tools/js-media-check.js user-apps/jsrun/src/bin/jsrun` (52 verificações) |
| Tudo junto no QEMU | `tools/perf/scen/w58-video.sh` (§14.5) |

### 15.5 Medidas no QEMU (TCG, sem KVM, BIOS, 256 MiB, AC'97 gravando em WAV)

`w58-video.sh` com `tools/w24-site.py` (páginas `/video` e `/mse` de `tools/w24media.py`; o vídeo é o do pedido:
`testsrc2` 640x360 30 fps VP9 300 kb/s + seno de 440 Hz em Opus, 3 s). **VERDICT: PASS**, 24 verificações:

* `/video`: o player nasce (processo `mediadec`), `canPlayType` = `probably`, `loadedmetadata` com 3,008 s, 640x360,
  `play()` resolve, toca até o fim (`ended` em 3 s), volta a 1 s (`seeked`) e toca até o fim de novo; um clique no
  play dos controles toca outra vez desde o começo. Duas capturas com 1 s de intervalo diferem em ~80 000 pixels (a
  imagem e a barra de tempo andam; `docs/img/browser-video.png`).
* `/mse`: dois `SourceBuffer`s (vídeo e som em arquivos WebM DASH separados, init + clusters de 1 s buscados com
  `Range`), `buffered` crescendo a cada segmento, `endOfStream` (duração 3,028 s), `sourceended`, toca, volta a 1 s,
  termina de novo.
* **O som que saiu** (gravado pelo QEMU, medido por `tools/perf/audio-check.py`): cinco sons de **440,00 Hz**, com
  **3000,0 ms**, **2001,0 ms** (depois do seek para 1 s), **3000,0 ms** (o play dos controles), **3000,0 ms** e
  **2008,0 ms** (MSE), cada um seguido de silêncio exato, 100 % da energia no tom, os dois canais iguais. Nenhuma
  falha de buffer do dispositivo.
* **O relógio**: tempo de mídia contra tempo real 1,001–1,008 (som) e 1,000 (sistema).
* **Velocidade**: o som é perfeito, mas a imagem não acompanha os 30 fps sob TCG: **11–13 quadros/s mostrados** no
  360p (os atrasados são descartados para manter a sincronia), com a CPU emulada em 100 %: decodificador **63–67 %**,
  compositor **31–35 %**. A decodificação VP9 sozinha a 360p faz ~46 fps sob TCG ([`midia.md`](midia.md)), então o
  decodificador já gasta ~2/3 da CPU só para acompanhar o vídeo. Em hardware real ou com KVM o VP9 360p decodifica a
  ~1100 fps (nativo, no hospedeiro), então sobra.
* **Sem placa de som** (`QEMU_EXTRA` vazio): `VERDICT: PASS` com o relógio do sistema; a imagem vai em trechos de
  ~15–22 fps entre esperas curtas (sem som não há o que manter em tempo, então o relógio espera as imagens).
* **Download**: 128 KiB do `src` em 76 ms pelo `fetcher` (1,6 MB/s, NE2000, um pedido `Range`).

**Um WebM real da internet** (`tools/perf/scen/w58-real.sh`, virtio-net, página `/real?src=...`):

* `https://test-videos.co.uk/.../Big_Buck_Bunny_360_10s_1MB.webm` (VP9 640x360, 10 s, sem som) direto: o proxy desta
  sandbox reassina o HTTPS e o certificado é recusado (`untrusted chain`); o elemento recebe `error` com
  `MEDIA_ERR_NETWORK` (2) e a página segue. Num computador com acesso direto à internet o mesmo pedido verifica a
  cadeia normalmente. (O Wikimedia Commons respondeu `429 Too Many Requests` ao IP desta sandbox, então não pôde ser
  usado.)
* O mesmo arquivo pelo servidor de teste (`/relay?u=...`: baixado uma vez pelo hospedeiro e servido com `Range`):
  1 052 940 bytes em 320–400 ms (2,6–3,2 MB/s), metadados 640x360/10 s, **tocou até o fim, voltou a 1 s e terminou
  de novo**. Sem som, com o relógio do sistema, o Big Buck Bunny (mais pesado de decodificar que o padrão de teste)
  decodifica mais devagar que o tempo real sob TCG (~5–7 quadros/s mostrados): 10 s de vídeo tocaram em ~30 s, com
  esperas.
* `flower.webm` do MDN (VP8 + Vorbis): `no VP9 or Opus track`, o elemento recebe `MEDIA_ERR_SRC_NOT_SUPPORTED` (4).

## 16. O YouTube no Navegador: o player embutido (bring-up)

Etapa 6 do item 11 do [`ROADMAP`](../ROADMAP.md): juntar as peças dos §12–15 com o YouTube de verdade. Alvo: a
página de embed `https://www.youtube.com/embed/aqz-KE-bpKQ` (Big Buck Bunny; ~140 KB de HTML, 15 scripts, entre eles
o `ytembeds` de 0,57 MB e o `base.js` do player de 1,9 MB) e, se desse, a `/watch`.

### 16.1 Até onde chegou (resumo honesto)

No QEMU (BIOS, 256 MiB, TCG, `tools/perf/scen/w60-youtube.sh`): o HTML do embed carrega, os 15 scripts rodam (o
`base.js` em 7,4 s), os scripts que o player insere carregam e rodam (módulos do `ytembeds`, o interpretador do
BotGuard de `www.google.com/js/th/`, que gera o token de integridade em `jnn-pa.googleapis.com`), o player monta e
pede `youtubei/v1/player` (200, 33 KB, `status: OK`), cria o `<video>` com `MediaSource` (o navegador abre o player e
o processo `mediadec`) e faz os pedidos de mídia `POST .../videoplayback` ao `googlevideo.com` (SABR: protocolo UMP,
com o PO token cunhado pelo BotGuard no corpo). **O vídeo não toca neste ambiente**: o `googlevideo` responde com um
redirecionamento e depois com `RELOAD_PLAYER_RESPONSE`, o player pede o `player` de novo e recebe `UNPLAYABLE`
("Vídeo indisponível / This content isn't available"), que ele desenha na tela. **A causa é a rede da sandbox, não o
navegador** (§16.4): o mesmo acontece, idêntico, rodando os mesmos scripts no V8 (Node) e no QuickJS do hospedeiro.
Por isso ficam **sem prova**: os bytes de mídia do YouTube chegando ao `SourceBuffer`, quadros na tela e som. O
caminho do MSE em si (WebM VP9/Opus em DASH, dois `SourceBuffer`s, som pelo AC'97) foi provado no W58 (§15).

**Atualização: um vídeo tocou.** Depois do resumo acima, o mesmo cenário (`w60-youtube.sh`, imagem com a raiz do gateway da
sandbox em `EXTRA_PEM`) foi repetido com vários vídeos (`VIDEO=<id>`): **`dQw4w9WgXcQ` passou em todas as etapas**: o
`googlevideo.com` entregou os segmentos (1 275 378 bytes num `videoplayback`), o `SourceBuffer` os recebeu, o
`mediadec` decodificou, o título do vídeo aparece na aba, as imagens mudam na tela (286 938 pixels diferentes em 1 s) e
o AC'97 gravou 65,4 s de som em 118,3 s (pico 16 526). `jNQXAC9IVRw`, `YE7VzlLtp-4` e `LXb3EKWsInQ` pararam no pedido da
mídia, com "O conteúdo não está disponível" desenhado pelo player. Ou seja, o bloqueio do §16.4 é intermitente e depende
do vídeo e do IP de saída daquele momento; o navegador cumpre o que a página do YouTube pede. Ainda sem medida: fps e
CPU da reprodução do YouTube no QEMU.

### 16.2 Método: a sonda no hospedeiro

`tools/yt-probe.js` (Node 22) faz o papel do navegador para o **processo de página de verdade**: `tools/jsrun-host.c`
compila o QuickJS, o `glue.c` e o `page.c` do `jsrun` para o hospedeiro (o `jsrun-host`, ~40 s com `clang`), com as
chamadas 56/57 trocadas por mensagens com tamanho em `stdin`/`stdout`. A sonda baixa a página e os scripts (curl,
pelo proxy da sandbox ou, com `--direct CA.pem`, pelo caminho que o QEMU usa), monta o `init` como o navegador
(inclusive os `<script>` vazios na árvore), entrega os scripts um por mensagem, atende o `F` (fetch/XHR) com uma
fila de um pedido por vez, um pote de cookies e as regras de CORS (com pré-voo), atende o `V`/`v` do MSE com um
decodificador falso que lê os tempos do WebM, manda um retrato de layout com caixas e imprime o console, os erros,
os scripts lentos, os pedidos e um resumo. `--engine node` roda o prelúdio num `vm` do V8 (outro motor: separa bug
do QuickJS de bug do prelúdio), `--spy` registra o que a página pergunta sobre suporte de mídia, `--play` clica no
player, `--referer` manda um `Referer`. Os pedidos do player e da mídia ficam em `--cache` (`dump-N.*`) para ler.
Nada do YouTube vai para o repositório.

```bash
node tools/yt-probe.js "https://www.youtube.com/embed/aqz-KE-bpKQ?autoplay=1" --referer http://203.0.113.5:8479/ --secs 40
```

### 16.3 Diário de bordo: o que o YouTube pediu e foi feito

| # | Sintoma (sonda ou QEMU) | Causa | Correção |
|---|---|---|---|
| 1 | O embed mostra "Error 153 / Video player configuration error" (a resposta do servidor já vem com `previewPlayabilityStatus: ERROR`) | o YouTube exige um `Referer` no embed (identifica o site que o embute); aberto pela barra de endereço não há, **em nenhum navegador** | a navegação por link, formulário ou `location.href` manda `Referer` (`strict-origin-when-cross-origin`, mantido na recarga; `Browser::referrer`, `fetch::try_post`), e a página o vê em `document.referrer`; a página `/yt` de `tools/w24-site.py` leva ao embed |
| 2 | `Uncaught Error: na` no `ytembeds` | o código procura o próprio `<script id=base-js>` (e `script[nonce]` para o nonce) e o Kitsune tirava os `<script>` da árvore | o parser mantém o elemento `<script>` (vazio, `display:none`, com o índice em `ScriptRef::elem`) e a página ganha `document.currentScript` |
| 3 | `base.js` (1,9 MB) recusado | limite de 1 MiB por script e 4 MiB por página, e todos os scripts dentro do `init` (JSON de até 4 MiB) | 4 MiB por script e 10 MiB por página; os scripts vão um por mensagem, código cru (sem escapar nem `JSON.parse` de megabytes), conforme o processo consome (`JsProc::scripts_left`); um script comprimido que não descompacta não roda |
| 4 | o player pede `/player` com um PO token "frio" e o `googlevideo` recarrega | o BotGuard nunca rodava: o `ytembeds` insere `<script src=//www.google.com/js/th/...>` e módulos seus, e o Kitsune não executava scripts inseridos | `scripts.js`: `<script>` inserido roda (inline na hora; `src` buscado pelo navegador no modo `script`); `cors::Mode::Script` no `fetcher`: sem CORS, mas só uma resposta com `Content-Type` JavaScript é entregue (como o ORB dos navegadores), e nunca ao código da página; `fetch()` recusa modos fora do padrão |
| 5 | `ReferenceError: 'Image' is not defined` num módulo | faltava o construtor | `compat.js`: `Image`, `Option`, `canvas.getContext` (devolve `null`: sem canvas ainda), `navigator.mediaCapabilities.decodingInfo`, `navigator.mediaSession`/`MediaMetadata`, `CDATASection`/`ProcessingInstruction` (a `/watch`) |
| 6 | o player perguntaria por 4K | `isTypeSupported` ignorava `width`/`height`/`framerate`; o player do YouTube sonda (`width=99999` deve ser recusado para ele confiar nos outros) | `isTypeSupported` respeita os parâmetros: até 854x480 a 30 fps (o que o VP9 em software acompanha mesmo sob TCG); `decodingInfo` usa o mesmo limite para `smooth` |
| 7 | `User-Agent` HTTP `Kitsune/0.1.0` e `navigator.userAgent` `Mozilla/5.0 (X11; Kitsune x86_64) Kitsune/0.1.0` | dois nomes | um só, honesto, nos dois (`http1::HTTP_USER_AGENT`) |
| 8 | no QEMU o `base.js` levou **62,2 s** (0,47 s nativo) e foi interrompido | o `malloc` do lado C do `jsrun` era o alocador first-fit em lista ligada do SDK; com a rotatividade do QuickJS cada `malloc` percorria uma lista longa | `heap.rs`: classes de tamanho de 16 bytes até 1 KiB (lista livre por classe, pedaços de 256 KiB); o `base.js` passou a **7,4 s**, o `ytembeds` de 6,2 s a 1,9 s, um módulo de 1 MB de 15,8 s a 2,3 s |
| 9 | carregar o `base.js` passa dos 3 s por tarefa mesmo assim | o QuickJS compila o arquivo inteiro de uma vez | carregar um script tem 60 s (`LOAD_MS` no `page.c`); eventos e timers continuam com 3 s; o interrupt handler é o mesmo |
| 10 | a mensagem "Ocorreu um erro... ative o JavaScript" debaixo do player | `<noscript>` era desenhado | `<noscript>` é `display:none` (o navegador roda scripts) |
| 11 | ~38 MB de memória do processo depois de carregar o embed (hospedeiro, `VmHWM`) | perto dos 48 MiB de JS | 96 MiB de JS e heap de até 128 MiB para o `jsrun` (só cresce com o uso) |
| 12 | (a `/watch`) scripts inline seguintes quebrados com `SyntaxError: '<'` | o parser de fragmentos achava o fim do `<script>` em `html.toLowerCase()`, que muda de tamanho com `İ` e deslocava os índices | busca com regex sem diferenciar maiúsculas a partir da posição |

Nada mais faltou ao player do embed: com essas correções os scripts rodam sem nenhuma exceção não capturada, na
sonda e no QEMU.

### 16.4 O bloqueio deste ambiente

* **Endereço IP.** As URLs de mídia do `googlevideo.com` são assinadas com o IP de quem pediu o `player`
  (`ip=`). Nesta sandbox o `www.youtube.com` vê o cliente por um endereço IPv6 **interno** (`fda3:e722:...`,
  `remoteHost` no próprio pedido do player: a sandbox roda dentro da rede do Google) e o `googlevideo.com` vê um
  IPv4 público (`mip=34.72.45.172`, `ipbypass=yes` no redirecionamento SABR), que além disso muda a cada conexão
  (três pedidos seguidos a um serviço de eco de IP deram três IPs). O servidor redireciona, depois pede ao player
  que recarregue, e a resposta recarregada é `UNPLAYABLE` (o relatório do próprio player:
  `error=staleconfig::reason.reloadPlayerEvent`, depois `auth::r.Vídeo indisponível`). Igual pelo proxy HTTP da
  sandbox e pelo caminho direto que o QEMU usa, igual no QuickJS e no V8. Num computador comum (um IP só), essa
  verificação passa.
* **HTTPS reassinado.** O tráfego do QEMU passa por um gateway que reassina o HTTPS (`sandbox-egress-gateway
  Egress Gateway CA`). Para a prova, a imagem foi gerada com essa raiz a mais (`EXTRA_PEM` de
  `tools/gen-trust-store.sh`, só para desenvolvimento, **nunca commitado**). No computador do dono a cadeia do
  Google (`GTS Root R1`/`R4`, que estão entre as 46 raízes) valida sem nada a mais.
* **A `/watch`** dá `429` (página de captcha do Google) pelo proxy HTTP da sandbox; pelo caminho direto responde
  `200`. Os limites dela estão em §16.6.

### 16.5 Medidas (QEMU TCG, sem KVM, BIOS, 256 MiB, virtio-net)

| O quê | Valor |
|---|---|
| Carga dos scripts do embed (compilar + nível de cima) | `ytembeds` 0,57 MB: 1,9 s; `base.js` 1,9 MB: 7,4 s; módulo de 1,1 MB inserido depois: 2,3 s; BotGuard 63 KB: 0,3 s (antes do novo `malloc`: 6,2 s, 62,2 s e 15,8 s). Nativo (hospedeiro, `jsrun-host`): 0,08–0,13 s, 0,35–0,47 s, 0,12 s |
| Rede | `youtubei/v1/player` 33 KB em 96 ms; módulo de 1 MB a 5,2 MB/s; 8 conexões e 7 handshakes TLS para ~20 pedidos |
| Memória | QEMU: o heap do processo da página passou de 32 MiB (`proc: 'jsrun' pid N heap 32 MiB`; não chegou a 48) com o player carregado e os pedidos feitos; hospedeiro: ~40 MB de pico do processo (`VmHWM`, binário incluso) |
| Outra rodada, com o hospedeiro ocupado (um `cargo clippy` em paralelo) | `base.js` em 11,6 s |

### 16.6 O que falta e os limites conhecidos

* **Tocar o vídeo do YouTube** não foi provado (§16.4). Falta ver, num lugar com um IP só: os segmentos SABR (UMP)
  que o JavaScript do player decodifica e apenda no `SourceBuffer`, o `mediadec` com os itags 243/244 (VP9) e 251
  (Opus), o relógio pelo som e o fps. O cenário `w60-youtube.sh` já mede tudo isso por etapa (bytes apendados,
  `playing`, imagens mudando, som na gravação).
* **`<iframe>`** não tem documento dentro: um site que embute o player numa página não funciona; o embed só abre
  como página de topo, vindo de um link (por causa do Error 153, digitar o endereço não serve).
* **A `/watch`** (sonda, caminho direto): 1,25 MB de HTML (o navegador corta páginas em 1 MiB), 52 scripts e
  5,6 MB no carregamento mais um módulo de 9,7 MB que o app insere (acima dos 4 MiB por script do navegador; na
  sonda ele estoura os 96 MiB de JS); o Polymer/webcomponents roda em parte. Sob TCG, compilar ~15 MB de JavaScript
  levaria mais de um minuto. Fica para depois: cache de bytecode, páginas maiores que 1 MiB, mais memória.
* Teto de 480p pelo `isTypeSupported` (com KVM ou em hardware real o decodificador faria 720p, §15.5); o `Referer` de
  um redirecionamento entre origens não é recortado para a origem; a história (voltar/avançar) não guarda o
  `Referer`.

### 16.7 Provas

| O quê | Evidência |
|---|---|
| `<script>` na árvore e `ScriptRef::elem`, `PageScript`, `init` com `wait` + `script_message`, `Mode::Script` e `is_javascript_type`, `Referer` de links/formulários/recarga, `<noscript>` escondido | `cargo test -p kitsune_core` (`script::tests`, `html_tests`, `net::tests`, `cors::tests`, `browser::referrer_tests`, `layout_tests::styles`) |
| O prelúdio com os scripts do YouTube, no QuickJS de verdade e no V8 | `node tools/yt-probe.js ...` (§16.2): 0 exceções, `player` 200, BotGuard, pedidos SABR com PO token |
| Nada quebrou no prelúdio | `node tools/js-dom-check.js`, `js-net-check.js`, `js-media-check.js` |
| Nada quebrou (mesma imagem: `<script>` na árvore, scripts por mensagem, `malloc` novo, `Referer`, `<noscript>`) | `w52-browser-js.sh` 19/19, `w58-video.sh` PASS (48 linhas, som medido), `w59-spa.sh` 24/24 |
| Tudo junto no QEMU | `tools/perf/scen/w60-youtube.sh`: PASS em 8 etapas (HTML, `base.js`, scripts entregues, scripts rodaram, scripts inseridos, `player`, `videoplayback`, `<video>` com player) e FAIL honesto em 4 (bytes no `SourceBuffer`, `playing`, imagens, som), pelo motivo do §16.4 |

Para tentar no computador do dono (Windows, WHPX, IP residencial): `python tools/w24-site.py 8479` no Windows e, no
Navegador do Kitsune, `http://10.0.2.2:8479/yt` (a rede padrão do QEMU), depois o link (ou `?go=1`).
