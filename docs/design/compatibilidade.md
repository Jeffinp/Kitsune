# Compatibilidade: apps, web e formatos

O que o Kitsune abre, roda e entende, e o que vem a seguir. Cada item tem um critério de aceite
verificável. O hardware tem seu próprio documento: [`../HARDWARE.md`](../HARDWARE.md).

## Apps

| Passo | O quê | Estado |
|---|---|---|
| A.1 | Plataforma de apps WebAssembly (manifesto, permissões, cotas, `/data/<id>`, instalação em `/apps`) | feito |
| A.2 | **SDK documentado**: tutorial "seu primeiro app em dez minutos" ([`SDK.md`](../SDK.md)), do código ao app rodando sem recompilar o sistema (compilar, `wget` no Terminal, Enter no Arquivos); referência do ABI em [`apps.md`](apps.md). | feito (o texto foi seguido de ponta a ponta em QEMU, W42) |
| A.3 | **WASI de verdade** sobre o OJFS: `wasi_snapshot_preview1` com `fd_*`, `path_*`, relógio e aleatoriedade, respeitando o sandbox do app; para rodar programas escritos em C, Rust ou Go sem o SDK. | a fazer (há uma base em `kernel/src/wasm/wasi.rs`) |
| A.4 | Vários apps ao mesmo tempo, um deles hostil, sem afetar os outros (item 5 do [roadmap](../ROADMAP.md)). | a fazer |
| A.5 | Combustível retomável: um quadro pesado legítimo deixa de ser encerrado. | a fazer |

*Aceite:* um programa em Rust ou C compilado para `wasm32-wasip1` roda no Terminal com acesso à sua pasta; o
tutorial do SDK produz um app instalável em menos de dez minutos seguindo só o texto.

## Web

| Passo | O quê | Estado |
|---|---|---|
| W.1 | HTML, CSS, imagens PNG/BMP/PPM, formulários **GET**, gzip/deflate, HTTPS verificado | feito |
| W.2 | **GIF** (com a primeira imagem de um GIF animado) e **JPEG** *baseline* | feito (JPEG progressivo também, até ~6 milhões de amostras; CMYK ainda não) |
| W.3 | Formulários **POST** (`application/x-www-form-urlencoded`), `<textarea>` e `<select>` | feito (`<select>` sem lista: o clique ou as setas trocam a opção; sem `multipart` nem envio de arquivos) |
| W.4 | *Redirect* de POST (303 vira GET; 307/308 mantêm o corpo) feito; reuso de conexão (*keep-alive*) a fazer | parcial |
| W.5 | HSTS, revogação e *pinning* (ver [usuários e segurança](usuarios-seguranca.md), fase 2) | a fazer |
| W.6 | JPEG CMYK, WebP, SVG simples | depois |

*Aceite:* uma página com um GIF e um JPEG mostra as duas imagens; um formulário POST com `<select>` e
`<textarea>` envia os campos e mostra a resposta (teste com o servidor local em `tools/`).

## Formatos de arquivo

| Passo | O quê | Estado |
|---|---|---|
| F.1 | Imagens: PNG, BMP, PPM; texto UTF-8 | feito |
| F.2 | Imagens: **GIF** e **JPEG** *baseline* também no app Imagens (mesmo decodificador do navegador) | feito |
| F.3 | Texto: detectar e converter Latin-1/Windows-1252 e UTF-16 (com marca) para UTF-8 ao abrir, e gravar de volta na mesma codificação (Editor, navegador); `iconv` no Terminal | feito (UTF-8 com BOM também) |
| F.4 | **ZIP** (listar, extrair e criar: `stored` e `deflate`) e `gzip` no Arquivos ("Extrair aqui", "Compactar em ZIP") e no Terminal (`zip`, `unzip`, `gzip`, `gunzip`) | feito (sem criptografia, ZIP64 nem `bzip2`) |
| F.5 | Fim de linha CRLF/LF tratado no Editor (já detecta e preserva) | feito |

*Aceite:* abrir um `.jpg` e um `.gif` no Imagens; abrir um `.txt` em Latin-1 no Editor sem texto
corrompido; extrair um `.zip` pelo Arquivos.

## Ordem

1. W.2/F.2 (GIF e JPEG): ganho imediato e visível, código puro e fuzzável no `kitsune_core`.
2. W.3 (POST, `<textarea>`, `<select>`): destrava formulários reais.
3. F.3 e F.4 (codificações de texto, ZIP).
4. A.2 e A.3 (SDK documentado e WASI), junto da fase 1 de usuários, porque o sandbox dos apps passa a
   usar as contas.
