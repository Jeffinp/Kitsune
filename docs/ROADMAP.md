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
| Navegador | HTML/CSS, imagens PNG/BMP/PPM, formulários GET, favoritos, busca na página, zoom, gzip/deflate, indicador de conexão segura |
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

### 10. Bloqueador de anúncios e rastreadores nativo (a fazer, pedido do dono do projeto)

O navegador deve trazer, de fábrica, um bloqueador de anúncios e rastreadores no estilo dos "Escudos" do Brave (só a
ideia: nada do código nem da marca do Brave). Decisões já tomadas:

- **A pessoa escolhe.** Ligado por padrão, com um botão na barra de endereço (e nas configurações) para desligar
  **por site**, por sessão ou para sempre. Se um site detectar o bloqueio e pedir para desativar, a pessoa desliga ali
  e a página recarrega sem o filtro e abre normalmente.
- **Como:** lista de regras embutida e atualizável (formato de filtros comum, só o subconjunto de bloqueio por domínio,
  caminho e tipo de recurso), aplicada **antes** do `fetcher` pedir qualquer subrecurso (imagens, folhas, scripts que
  nem rodamos) e na diagramação (elementos escondidos por seletor cosmético). Sem JavaScript no navegador, o
  ganho é menos tráfego, menos memória e páginas mais limpas.
- **Extras do estilo Brave, em ordem:** bloquear rastreadores de terceiros, remover parâmetros de rastreio das
  URLs (`utm_*`, `fbclid`...), atualizar http→https quando o site oferece, contador de itens bloqueados na página.
- **Aceite:** uma página de teste com anúncios e rastreadores locais não pede nenhum deles com o filtro ligado e pede
  todos com ele desligado; o botão desliga só o site atual; testes no host (`kitsune_core`) das regras e do casamento
  de URLs, e uma cena no QEMU.
- Ver `docs/design/tls-browser.md` §11.

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
2. **DOM e APIs da web** sobre o `Doc`: `document`, `querySelector`, eventos, `setTimeout`, `fetch`/`XMLHttpRequest`
   sobre o `fetcher`, `localStorage`, `history`, cookies (hoje não há). Aceite: uma página com JavaScript que monta a si
   mesma (um app pequeno de React/Preact) aparece certa.
3. **Rede para o que o YouTube pede:** cookies, HTTP keep-alive/HTTP/2 se o servidor exigir, CORS, WebSocket.
4. **Mídia:** Media Source Extensions, decodificadores de vídeo (H.264 e VP9 em Rust puro, em software: a CPU em
   software de 1080p não dá; 360p/480p é a meta realista) e de áudio (AAC e Opus), `<video>`/`<audio>`, sincronia.
5. **Som:** driver de áudio (Intel HDA ou AC'97/virtio-snd no QEMU) e mixagem; hoje o sistema não tem som.
6. **O próprio YouTube:** o player pede dezenas de APIs (Web Workers, `requestAnimationFrame`, `MutationObserver`,
   `IntersectionObserver`, Canvas, WebAudio, DRM não, mas vários recursos de segurança). Aceite: abrir um vídeo e vê-lo
   e ouvi-lo por um minuto.

Sem a etapa 1 nada do resto roda; a ordem acima é a mínima. Enquanto isso o navegador continua ganhando o que páginas
sem JavaScript pedem (itens do §10 do `tls-browser.md`).

### 12. Mais adiante: SMP, USB e som
Vários núcleos (APIC), prioridades no scheduler, USB (teclado, mouse e armazenamento) e áudio.
*Aceite:* cada um com sua prova (o desktop usa mais de um núcleo; um teclado USB funciona).
