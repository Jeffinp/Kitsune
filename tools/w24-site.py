#!/usr/bin/env python3
"""A local test site for the browser's page rendering (W24): typography, lists, tables,
forms, images, long words, CJK/RTL strings, CSS and big pages.

Same setup as tools/nettest-pages.py (a public-looking SLIRP range whose host alias is this
server):

    tools/w24-site.py &                       # 127.0.0.1:8079 (HTTP)
    QEMU_NETDEV="user,id=n0,net=203.0.113.0/24,host=203.0.113.5,dhcpstart=203.0.113.15,dns=203.0.113.3" \\
      tools/perf/run.sh <img> bios <out> 200 tools/perf/scen/w24-pages.sh

Routes: / (index), /type, /lists, /tables, /forms, /result (echoes the query), /images,
/long, /intl, /styled, /quote, /js (JavaScript: W52), /big?n=2000 (about n nodes), /slow (a 3 s response, to see the
progress bar), /redirect, /gone (404), /hang (never answers).

/ads is the page for the ad blocker (W53): it asks for a style sheet, three pictures and two tracking
pixels that the blocker's path rules catch (/ads/..., /pagead/..., /tracking/pixel...), two pictures from
the reserved .invalid names the rule lists carry for tests (they never resolve, so they only show up
in the guest's log), and two ad boxes that the cosmetic rules hide (one holds a picture that must then
not be asked for). With the blocker on the server sees only /ads and /g.png; with it off, all of them.
Every request is printed on stderr, so `tools/w24-site.py 2> requests.log` is the record to check.
"""
import struct
import sys
import time
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

PORT = 8079


def png(w, h, fn):
    """A w x h RGB PNG whose pixel (x, y) is fn(x, y)."""
    raw = bytearray()
    for y in range(h):
        raw.append(0)
        for x in range(w):
            raw.extend(fn(x, y))

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(bytes(raw), 6))
        + chunk(b"IEND", b"")
    )


def gradient(x, y):
    return (40 + x * 200 // 320, 80 + y * 120 // 160, 220 - x * 150 // 320)


def checker(x, y):
    on = ((x // 16) + (y // 16)) % 2
    return (0x5B, 0x5C, 0xF6) if on else (0xE9, 0xE9, 0xFB)


PAGE = """<!doctype html><html lang="pt-BR"><head><meta charset="utf-8"><title>{title}</title></head>
<body>{body}</body></html>"""

NAV = (
    '<p><a href="/">Início</a> · <a href="/type">Tipografia</a> · <a href="/lists">Listas</a> · '
    '<a href="/tables">Tabelas</a> · <a href="/forms">Formulários</a> · <a href="/images">Imagens</a> · '
    '<a href="/long">Palavras longas</a> · <a href="/intl">Idiomas</a> · <a href="/styled">Estilos</a> · '
    '<a href="/big">Grande</a></p>'
)


def page(title, body):
    return PAGE.format(title=title, body=NAV + body).encode()


LOREM = (
    "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut "
    "labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco "
    "laboris nisi ut aliquip ex ea commodo consequat."
)
PT = (
    "A rápida raposa marrom pula sobre o cão preguiçoso; açúcar, coração, ação, órgão, avô, "
    "também, você, maçã, lição e pingüim. “Aspas”, ‘simples’, travessão — e reticências…"
)


JS_PAGE = """<!doctype html><html><head><meta charset=utf-8><title>Sem JavaScript</title>
<style>
body{font-family:sans-serif;margin:16px;background:#fafafa}
.box{padding:12px;background:#e8e4ff;border-radius:8px;margin:8px 0}
.on{background:#bdf5c9;font-weight:bold}
button{margin:4px 8px 4px 0}
#list li{color:#5b3df5}
</style>
<script src="/js/app.js"></script>
</head><body>
<h1 id=title>Sem JavaScript</h1>
<p id=status>O script ainda não rodou.</p>
<ul id=list></ul>
<p><button id=btn>Clique aqui</button><button id=loop>Laço infinito</button></p>
<p id=count class=box>Cliques: 0</p>
<p id=timer>Timer: 0</p>
<p><a id=prevent href="/type">Link que o script segura</a> · <a href="javascript:mark('js-link')">link javascript:</a></p>
<p id=log>-</p>
<script>
  // Inline, after the external one: it already sees what app.js defined.
  console.log('js: inline sees app =', typeof window.app, 'readyState', document.readyState);
  document.getElementById('status').textContent = 'Script inline rodou.';
</script>
</body></html>"""

JS_APP = r"""
'use strict';
window.app = { clicks: 0 };
function mark(what) {
  document.getElementById('log').textContent = 'último: ' + what;
  console.log('js: mark ' + what);
}
console.log('js: app.js start, ua=' + navigator.userAgent.split(' ')[0] + ' host=' + location.host +
  ' path=' + location.pathname);
document.addEventListener('DOMContentLoaded', () => {
  console.log('js: DOMContentLoaded, items before:', document.querySelectorAll('#list li').length);
  document.title = 'Com JavaScript';
  document.getElementById('title').textContent = 'Montada pelo JavaScript';
  const list = document.querySelector('#list');
  for (const name of ['um', 'dois', 'três']) {
    const li = document.createElement('li');
    li.textContent = 'Item ' + name + ' (criado com createElement)';
    li.setAttribute('data-n', name);
    list.appendChild(li);
  }
  list.insertAdjacentHTML('beforeend', '<li><b>Item quatro</b> (innerHTML)</li>');
  console.log('js: list has', list.children.length, 'items; matches', list.matches('ul#list'),
    'closest', list.querySelector('b').closest('ul').id);
  const btn = document.getElementById('btn');
  btn.addEventListener('click', (e) => {
    app.clicks++;
    const c = document.getElementById('count');
    c.textContent = 'Cliques: ' + app.clicks;
    c.classList.toggle('on');
    console.log('js: click', app.clicks, 'target', e.target.id, 'class', c.className);
  });
  document.getElementById('prevent').onclick = (e) => {
    e.preventDefault();
    mark('link segurado');
  };
  document.getElementById('loop').addEventListener('click', () => {
    console.log('js: entering an endless loop');
    for (;;) {}
  });
  let ticks = 0;
  const iv = setInterval(() => {
    ticks++;
    document.getElementById('timer').textContent = 'Timer: ' + ticks;
    if (ticks === 3) { clearInterval(iv); console.log('js: interval done'); }
  }, 300);
  setTimeout(() => {
    document.getElementById('status').innerHTML = 'Mudado pelo <i>setTimeout</i>.';
    console.log('js: timeout fired');
  }, 600);
  requestAnimationFrame((t) => console.log('js: animation frame', t >= 0));
  Promise.resolve().then(() => console.log('js: microtask'));
});
window.addEventListener('load', () => console.log('js: load event'));
"""


def route(path, q):
    if path == "/":
        return page(
            "Site de testes",
            "<h1>Site de testes do navegador</h1><p>Páginas para conferir a tipografia, as listas, "
            "as tabelas, os formulários e as imagens.</p>" + "".join(
                f'<p><a href="{p}">{p}</a></p>'
                for p in ["/type", "/lists", "/tables", "/forms", "/images", "/long", "/intl", "/styled", "/quote", "/big", "/slow", "/gone", "/ads"]
            ),
        )
    if path == "/type":
        return page(
            "Tipografia",
            f"""<h1>Título nível 1</h1><h2>Título nível 2</h2><h3>Título nível 3</h3>
<h4>Título nível 4</h4><h5>Título nível 5</h5><h6>Título nível 6</h6>
<p>{PT}</p><p>{LOREM}</p>
<p>Texto com <b>negrito</b>, <i>itálico</i>, <b><i>negrito itálico</i></b>, <u>sublinhado</u>,
<s>riscado</s>, <code>código inline</code>, <mark>marcado</mark>, <small>pequeno</small>,
<big>grande</big>, x<sub>2</sub> e x<sup>2</sup>, e um <a href="/type">link</a> no meio de uma frase
longa que quebra de linha para mostrar como os trechos de estilos diferentes se encaixam na mesma
linha sem espaços sobrando nem faltando.</p>
<p style="text-align:center">Parágrafo centralizado com mais de uma linha, para ver o alinhamento: {LOREM}</p>
<p style="text-align:right">Parágrafo à direita.</p>
<p style="line-height:2">Entrelinha dupla. {LOREM}</p>
<p style="font-size:22px;font-weight:600">Texto grande e semibold (22 px).</p>
<pre>fn main() {{
    println!("pré-formatado");   // mantém    os espaços
}}</pre>
<blockquote>Uma citação em bloco: {LOREM}</blockquote>
<hr>
<p>Depois da régua. Palavras acentuadas: ÀÁÂÃÄÅ ÇÈÉÊË ÌÍÎÏ ÑÒÓÔÕÖ ÙÚÛÜ Ýàáâãäå çèéêë ìíîï ñòóôõö ùúûü ýÿ.</p>""",
        )
    if path == "/lists":
        return page(
            "Listas",
            """<h1>Listas</h1><ul><li>Primeiro item</li><li>Segundo item com um texto bem longo que
precisa quebrar de linha para mostrar o recuo da segunda linha alinhada ao texto e não ao marcador
da lista, como nos navegadores de verdade.</li><li>Terceiro<ul><li>Aninhado A<ul><li>Aninhado mais
fundo</li></ul></li><li>Aninhado B</li></ul></li></ul>
<ol><li>Um</li><li>Dois</li><li>Três<ol type="a"><li>Sub um</li><li>Sub dois</li></ol></li></ol>
<ol start="8"><li>Oito</li><li>Nove</li><li>Dez</li><li>Onze</li></ol>
<dl><dt>Termo</dt><dd>Definição do termo, com mais de uma palavra.</dd><dt>Outro</dt><dd>Outra definição.</dd></dl>""",
        )
    if path == "/tables":
        return page(
            "Tabelas",
            """<h1>Tabelas</h1>
<table border="1"><caption>Notas do semestre</caption>
<thead><tr><th>Aluno</th><th>Matéria</th><th>Nota</th></tr></thead>
<tbody><tr><td>Ana Paula</td><td>Matemática</td><td>9,5</td></tr>
<tr><td>Bruno</td><td>História do Brasil e Geografia Geral</td><td>7,0</td></tr>
<tr><td>Carla</td><td>Física</td><td>8,25</td></tr></tbody></table>
<p>Tabela sem bordas, com largura total:</p>
<table style="width:100%"><tr><td>Esquerda</td><td style="text-align:center">Centro</td><td style="text-align:right">Direita</td></tr>
<tr><td colspan="2">Célula de duas colunas</td><td>Fim</td></tr></table>
<p>Tabela estreita com texto longo:</p>
<table style="width:300px;background:#f3f3f8"><tr><td>Uma célula com bastante texto que precisa quebrar dentro da coluna.</td><td>Outra célula também comprida para disputar a largura.</td></tr></table>""",
        )
    if path == "/forms":
        return page(
            "Formulários",
            """<h1>Formulário</h1><form action="/result" method="get">
<p>Nome: <input name="nome" value="José"></p>
<p>Busca: <input type="search" name="q" placeholder="Digite para buscar"></p>
<p>Senha: <input type="password" name="senha" value="segredo"></p>
<p><input type="checkbox" name="a" value="1" checked> Aceito os termos
<input type="checkbox" name="b" value="2"> Quero novidades</p>
<p><input type="radio" name="cor" value="azul" checked> Azul <input type="radio" name="cor" value="verde"> Verde
<input type="radio" name="cor" value="rosa"> Rosa</p>
<input type="hidden" name="origem" value="w24">
<p><input type="submit" value="Enviar"> <button type="button">Botão sem ação</button></p></form>
<form action="/result" method="post"><p><input name="x"><button>Enviar (POST)</button></p></form>""",
        )
    if path == "/result":
        items = "".join(f"<li><b>{k}</b> = {v[0]}</li>" for k, v in q.items())
        return page("Resultado", f"<h1>Recebido</h1><ul>{items}</ul>")
    if path == "/images":
        return page(
            "Imagens",
            """<h1>Imagens</h1><p>Gradiente 320x160:</p><img src="/g.png" width="320" height="160" alt="gradiente">
<p>Xadrez, sem tamanho declarado:</p><img src="/c.png" alt="xadrez">
<p>Uma imagem larga demais para a coluna (1400x200):</p><img src="/wide.png" alt="larga">
<p>Imagem como link: <a href="/type"><img src="/c.png" width="64" height="64" alt="xadrez"></a> e texto depois.</p>
<p>Quebrada: <img src="/nope.png" alt="Imagem que não existe" width="200" height="80"></p>""",
        )
    if path == "/long":
        w = "Pneumoultramicroscopicossilicovulcanoconiótico" * 4
        return page(
            "Palavras longas",
            f"""<h1>Palavras longas</h1><p>{w}</p>
<p>https://exemplo.com.br/um/caminho/muito/comprido/que/nao/tem/espacos/para/quebrar/a/linha/ate/o/fim/da/coluna/{'x' * 120}</p>
<p>{'a' * 400}</p><p style="width:120px;background:#eee">Palavra estreita demais: Supercalifragilisticexpialidocious</p>""",
        )
    if path == "/intl":
        return page(
            "Idiomas",
            """<h1>Idiomas</h1><p>Português: ação, coração, maçã.</p>
<p>Chinês: 你好，世界。这是一个很长的中文句子，用来测试在没有空格的情况下是否可以在任意字符之间换行，而且不会崩溃。</p>
<p>Japonês: こんにちは世界。これはテスト用の長い文章です。</p>
<p>Coreano: 안녕하세요 세계. 이것은 테스트용 문장입니다.</p>
<p>Árabe: مرحبا بالعالم، هذه جملة اختبار.</p><p>Hebraico: שלום עולם, זהו משפט לבדיקה.</p>
<p>Russo: Привет, мир! Это тестовое предложение.</p><p>Grego: Γειά σου κόσμε.</p>
<p>Emoji: 😀 🚀 🇧🇷 👨‍👩‍👧 ❤️ ✅</p><p>Polonês: zażółć gęślą jaźń.</p>
<p>Controles: a​b‎c‮d (zero-width, marcas bidi)</p>""",
        )
    if path == "/styled":
        return page(
            "Estilos",
            """<style>
body{background:#f7f7fb;color:#222}
.wrap{max-width:560px;margin:0 auto}
.card{background:#fff;border:1px solid #dcdce6;border-radius:12px;padding:16px 20px;margin:16px 0}
.card h2{margin-top:0;color:#4f46e5}
.btn{display:inline;background:#4f46e5;color:#fff;padding:4px 10px;border-radius:6px}
.note{border-left:4px solid #f59e0b;background:#fff7e6;padding:8px 12px}
.dark{background:#16161d;color:#e8e8f0;padding:12px}
.dark a{color:#9aa0ff}
a:hover{color:red}
</style><div class="wrap"><h1>Cartões</h1>
<div class="card"><h2>Primeiro cartão</h2><p>Texto dentro de um cartão com borda arredondada e fundo branco.</p><p><span class="btn">Botão</span></p></div>
<div class="card"><h2>Segundo cartão</h2><p class="note">Uma nota com barra lateral âmbar.</p><ul><li>Item um</li><li>Item dois</li></ul></div>
<div class="dark"><p>Bloco escuro com <a href="/styled">link claro</a>.</p></div>
<p style="margin:0 auto;width:50%;background:#e0e7ff;text-align:center">Largura 50% centralizada</p></div>""",
        )
    if path == "/modern":
        # The styles live in a separate sheet (/modern.css): the browser has to fetch it.
        return (
            "<!doctype html><html><head><meta charset=utf-8><title>Moderna</title>"
            "<link rel=stylesheet href=/modern.css></head><body>"
            "<header class=bar><span class=brand>Kitsune</span><nav><a href=/>Início</a><a href=/type>Tipos</a>"
            "<a href=/lists>Listas</a></nav></header>"
            "<main><img class=pic src=/c.png width=160 height=96 alt=''>"
            f"<h1>Página com CSS externo</h1><p>{LOREM}</p><p>{LOREM}</p>"
            "<section class=cards><article class=card><h2>Um</h2><p>Primeiro cartão da grade.</p></article>"
            "<article class=card><h2>Dois</h2><p>Segundo cartão.</p></article>"
            "<article class=card><h2>Três</h2><p>Terceiro cartão.</p></article>"
            "<article class=card><h2>Quatro</h2><p>Quarto cartão.</p></article></section>"
            "<div class=cols><div class=col>Coluna A</div><div class=col>Coluna B</div><div class=col>Coluna C</div></div>"
            "<p class=after>Depois das colunas flutuantes.</p></main>"
            "<footer class=foot><span>Rodapé</span><span class=push>fim</span></footer></body></html>"
        ).encode()
    if path == "/js":
        # A page that builds part of itself with JavaScript (/js/app.js and an inline script) and
        # answers clicks: W52.
        return JS_PAGE.encode()
    if path == "/bigcss":
        # A 300 KB sheet (served gzip-compressed): the rule that matters is the last one.
        return (
            "<!doctype html><html><head><meta charset=utf-8><title>CSS grande</title>"
            "<link rel=stylesheet href=/big.css></head><body><p class=first>Primeiro</p>"
            "<p class=last>Ultimo: vermelho se a folha chegou inteira.</p></body></html>"
        ).encode()
    if path == "/ads":
        # Expected with the blocker on: ads=4 (1 sheet, 3 pictures), trackers=2, hidden=2.
        return (
            "<!doctype html><html lang=pt-BR><head><meta charset=utf-8><title>Bloqueador</title>"
            "<link rel=stylesheet href=/ads/ad-theme.css></head><body>"
            "<h1>Página de teste do bloqueador</h1>"
            "<p>O título fica vermelho só se a folha /ads/ad-theme.css (um anúncio) for carregada.</p>"
            "<p>Imagem do próprio site (sempre aparece):</p><img src=/g.png width=160 height=80 alt=site>"
            "<p>Anúncio por caminho (/ads/banner):</p><img src=/ads/banner-728.png width=364 height=45 alt=anuncio>"
            "<p>Anúncio por caminho (/pagead/):</p><img src=/pagead/ad.png width=364 height=45 alt=anuncio>"
            "<p>Anúncio de rede (host de teste):</p><img src=http://kitsune-ads.invalid/banner.png width=300 height=60 alt=rede>"
            "<p>Pixels de rastreio:</p><img src='/tracking/pixel.png?id=7' width=1 height=1 alt=''>"
            "<img src=http://kitsune-tracker.invalid/pixel.gif width=1 height=1 alt=''>"
            "<div class=ad-banner>Caixa de anúncio escondida"
            "<img src='/c.png?in-hidden-ad' width=100 height=40 alt=''></div>"
            "<ins class=adsbygoogle>Outra caixa de anúncio</ins>"
            f"<p>{LOREM}</p><p>FIM DA PÁGINA</p></body></html>"
        ).encode()
    if path == "/quote":
        return page("Citações", f"<h1>Citações</h1><blockquote>{LOREM}<blockquote>Aninhada: {LOREM}</blockquote></blockquote>")
    if path == "/big":
        n = int(q.get("n", ["2000"])[0])
        rows = "".join(
            f"<li>Linha {i}: <b>negrito</b> e <a href='/big?n={n}'>link</a> com texto {LOREM[:60]}</li>"
            for i in range(n // 6)
        )
        return page("Página grande", f"<h1>Página grande ({n} nós)</h1><ul>{rows}</ul><p>FIM DA PÁGINA</p>")
    if path == "/slow":
        time.sleep(3)
        return page("Lenta", "<h1>Resposta lenta</h1><p>Esta página demorou três segundos.</p>")
    return None


class H(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        print(self.address_string(), fmt % args, file=sys.stderr, flush=True)

    def reply(self, code, ctype, body, extra=()):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        for k, v in extra:
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        u = urlparse(self.path)
        q = parse_qs(u.query)
        if u.path == "/g.png":
            return self.reply(200, "image/png", png(320, 160, gradient))
        if u.path == "/c.png":
            return self.reply(200, "image/png", png(160, 96, checker))
        if u.path == "/wide.png":
            return self.reply(200, "image/png", png(1400, 200, gradient))
        if u.path in ("/ads/banner-728.png", "/pagead/ad.png"):
            return self.reply(200, "image/png", png(364, 45, checker))
        if u.path == "/tracking/pixel.png":
            return self.reply(200, "image/png", png(1, 1, lambda x, y: (0, 0, 0)))
        if u.path == "/ads/ad-theme.css":
            return self.reply(200, "text/css", b"h1{color:#c0392b}")
        if u.path == "/modern.css":
            css = """
:root{--brand:#5b3df5;--gap:12px;--radius:10px}
*{box-sizing:border-box}
body{margin:0;font-family:sans-serif;color:#222;background:#fafafa}
.bar{display:flex;align-items:center;justify-content:space-between;padding:0 20px;height:56px;background:var(--brand);color:#fff}
.brand{font-weight:bold;font-size:20px}
.bar nav{display:flex;gap:var(--gap)}
.bar a{color:#fff;text-decoration:none;padding:6px 10px;border-radius:6px;background:rgba(255,255,255,.18)}
main{max-width:min(100%, 760px);margin:0 auto;padding:20px}
.pic{float:right;margin:0 0 10px 16px;border-radius:8px}
.cards{display:grid;grid-template-columns:repeat(2,1fr);gap:var(--gap);clear:both;margin:24px 0}
.card{background:#fff;border:1px solid #ddd;border-radius:var(--radius);padding:calc(var(--gap) + 4px)}
.card h2{margin:0 0 6px;color:var(--brand)}
.cols::after{content:"";display:table;clear:both}
.col{float:left;width:33.333%;padding:10px;background:#e8e4ff;border:2px solid #fafafa}
.after{margin-top:20px}
.foot{display:flex;padding:14px 20px;background:#222;color:#ccc}
.push{margin-left:auto}
@media (min-width: 1000px){ .cards{grid-template-columns:repeat(4,1fr)} }
@media (max-width: 500px){ .cards{grid-template-columns:1fr} }
@media print{ body{display:none} }
"""
            return self.reply(200, "text/css; charset=utf-8", css.encode())
        if u.path == "/js/app.js":
            return self.reply(200, "text/javascript; charset=utf-8", JS_APP.encode())
        if u.path == "/big.css":
            import gzip
            rules = "".join(f".r{i}{{color:#{i % 4096:03x};margin:{i % 7}px;padding:{i % 5}px}}\n" for i in range(9000))
            css = (rules + ".first{color:#00aa00}.last{color:#ff0000;font-weight:bold}\n").encode()
            if "gzip" in self.headers.get("Accept-Encoding", ""):
                return self.reply(200, "text/css", gzip.compress(css), [("Content-Encoding", "gzip")])
            return self.reply(200, "text/css", css)
        if u.path == "/redirect":
            return self.reply(302, "text/plain", b"", [("Location", "/type")])
        if u.path == "/hang":
            time.sleep(120)
            return
        body = route(u.path, q)
        if body is None:
            return self.reply(404, "text/html; charset=utf-8", page("Não encontrado", "<h1>404</h1><p>Não encontrado.</p>"))
        self.reply(200, "text/html; charset=utf-8", body)

    def do_POST(self):
        self.reply(200, "text/html; charset=utf-8", page("POST", "<p>POST recebido</p>"))


if __name__ == "__main__":
    srv = ThreadingHTTPServer(("0.0.0.0", PORT), H)
    print(f"w24-site on :{PORT}", file=sys.stderr, flush=True)
    srv.serve_forever()
