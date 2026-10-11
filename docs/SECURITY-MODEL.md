# Modelo de segurança do Kitsune

Este documento diz **o que o Kitsune protege, contra quem, e o que não protege**.
Foi escrito depois de uma auditoria completa (ver [`audit/`](audit/RELATORIO.md))
e cada afirmação aponta para uma prova: teste, fuzzing, medição em QEMU ou o
trecho de código. O que não foi provado está marcado como *suposição*.

## Resumo em cinco linhas

1. **Tudo roda em ring 0, num único espaço de endereçamento.** Não existe processo
   de usuário: um bug em qualquer componente é um bug no kernel inteiro.
2. Por isso a defesa é **na entrada**: todo dado que vem de fora (rede, disco,
   HTML/CSS, `.wasm`) passa por código que não usa `unsafe`, tem limites
   explícitos e foi fuzzado.
3. O que ainda é fraco e conhecido: não há isolamento entre apps e kernel (a fronteira é o
   WebAssembly com limites), o HTTPS não tem revogação nem *pinning* (§3.1) e os apps agora
   têm disco e rede reais, com as regras do §3.6.
4. Falhas fatais são **visíveis** (tela de erro + serial) e uma thread secundária que
   falha **morre sozinha** em vez de derrubar a máquina.
5. Nenhuma garantia vale em hardware real: tudo foi verificado em QEMU.

## 1. Fronteiras de confiança

Quem controla o dado, quem o interpreta e o que impede o pior.

| # | Fronteira | Quem controla o dado | Quem interpreta | Proteções | Risco residual |
|---|---|---|---|---|---|
| 1 | **Frames de rede** (virtio-net, NE2000) | qualquer host na rede | `kitsune_core::{net, lease, dns, icmp}` (ARP, IPv4, ICMP, UDP, DHCP, DNS) e `smoltcp` | `forbid(unsafe_code)` nesses módulos; fuzz de `net_parse` cobre o parser e responder, a máquina de lease, a resposta DNS e o ICMP (38 M execuções sem crash antes da rodada de rede; 1,2 M, 90 s, depois dela); resposta DNS só vale com id, pergunta e origem certos e registros do dono certo; ICMP de erro só vale se citar o nosso pedido; orçamento de 32 frames por acordada | `smoltcp` e os drivers não são fuzzados; DHCP e DNS sem autenticação; id e porta de origem do DNS (e o ISN do TCP) vêm do gerador do kernel (§3.7), tão bons quanto a nota dele; frame que cruza o fim do anel do DP8390 *(suposição, não provado)* |
| 2 | **Respostas HTTP/HTTPS** | servidor remoto, ou quem estiver no caminho | `fetch` → `kitsune_core::browser` (cabeçalhos, `chunked`, redirect) | corpo limitado a 1 MiB (`MAX_RESPONSE_BYTES`, ainda compactado) nos dois protocolos e 4 MiB descompactado (gzip/deflate, também em cadeia até 4 codificações; `br`/`zstd` nunca são oferecidos nem aceitos); um corpo compactado cortado ou danificado mostra o que foi decodificado, com aviso, em vez de erro; `dechunk` sem panic; redirect sem rebaixar https→http, no máximo 5 saltos, rejeita caracteres de controle; **TLS com cadeia de certificado, nome e `CertificateVerify` verificados** contra uma trust store embutida (§3.1) | sem revogação (CRL/OCSP), *pinning* nem HSTS (§3.1) |
| 3 | **HTML e CSS** | página remota | `kitsune_core::web` | profundidade 40, 8 000 nós, 1 000 regras e 2 000 seletores; comprimentos CSS limitados; cores não-ASCII rejeitadas; fuzz de ~0,7 M execuções sem crash | cascata ainda é O(regras × elementos) dentro dos tetos; fuzz do `web` ainda ganhava cobertura quando parou |
| 4 | **Disco (OJFS)** | quem fornecer a imagem | `kitsune_core::fs` | validação de `size`, `parent` (ciclos), imagem curta; fuzz de 1 M execuções sem crash; falha de leitura **não** reescreve o disco | disco com conteúdo desconhecido ainda é formatado (é o desenho do disco dedicado); sem permissões nem criptografia |
| 5 | **Apps `.wasm`** | embutidos no build, **ou instalados pelo usuário** (um `.wasm` aberto no Arquivos; o manifesto pede permissões e cotas e o instalador recusa o que passa do teto) | `wasmi` + as host functions `host.*`/`osj.*`; `kitsune_core::{wasmsec, appmanifest, appfs, appnet, appinstall}` | combustível por chamada (20 M; 256 M na inicialização), memória de 24 MiB, tetos nas host functions, o app é encerrado e liberado em qualquer falha; arquivos só em `/data/<id>` ou `/home` conforme `fs=`, com cota; rede só com `net=http`, destinos públicos e `net_hosts` (§3.6) | o TCB inclui `wasmi` e as host functions: um bug ali é fuga total; não há *fuel* retomável (um quadro pesado legítimo é encerrado); sem assinatura de pacotes: quem instala um `.wasm` concede o que o manifesto pede |
| 5b | **Dados persistentes dos apps** (`/apps`, `/data/<id>`, `/home` no disco OJFS v3) | os apps (conteúdo), o usuário (pacotes) | `kitsune_core::appfs::{Sandbox, VolumeFs}` sobre o `Backend` do VFS | caminho nunca concatenado (componentes validados, `..` acima da raiz é erro); só as três árvores da plataforma são alcançáveis pelo adaptador (`/etc`, `/var`, `/.trash`, pastas do usuário: `ERR_PERM`); cota por app (inclui o que já estava em disco); arquivo <= 64 MiB; fuzz `app_sandbox` sobre `MemFs` e sobre `VolumeFs`; testes de remount e de disco cheio | `fs=home` dá acesso a **tudo** em `/home` (é a pasta do usuário, por definição); apps com `fs=own` deixam dados no disco mesmo depois de removidos; nenhum apagamento seguro; ver §3.6 |
| 6 | **Dispositivos** (PS/2, virtio, ATA) | hardware | drivers do kernel | `virtio` valida `qsize`, BAR e limites de capability antes de tocar MMIO (testado no host) | dispositivos são confiáveis por premissa; DMA do virtio-gpu não tem IOMMU |
| 7 | **Firmware/bootloader** | fabricante | `bootloader 0.11` | — | imagem **não assinada**: Secure Boot precisa estar desligado |

## 2. O que o kernel faz para se defender

| Mecanismo | Estado | Prova |
|---|---|---|
| GDT/TSS próprias com pilha IST para #DF | feito (`gdt.rs`) | estouro de pilha vira `#DF` reportado, sem triple fault (BIOS e UEFI, `-d cpu_reset`) |
| Todas as exceções da CPU têm handler | feito (`interrupts.rs`) | `ud2`, `#PF`, `#NM`, `#XM` e outras exercitados com ganchos de build |
| Tela de erro + serial em panic/exceção/OOM | feito (`crash.rs`) | capturas em `docs/img/panic-*.png` |
| IRQ espúria 7/15 ignorada | feito | logado em QEMU (corrigiu um boot quebrado com `virtio-gpu-pci`) |
| W^X na imagem do kernel, NX no resto | herdado do bootloader | escrever em `.text` e executar no heap dão `#PF` (medido na auditoria) |
| `unsafe` documentado | 100% dos blocos com `// SAFETY:`, imposto pelo clippy | `cargo lint-kernel` (`-D warnings`) |
| Core sem `unsafe` | `#![forbid(unsafe_code)]` | compilação |
| Página de guarda nas pilhas das threads | feito (`vm.rs`, `sched::spawn`) | estouro em `fetcher`/`wasmapp` (recursão e frame de 150 KiB) cai na guarda, `#PF` em pilha IST, só a thread morre; antes, corrompia o heap (BIOS e UEFI) |
| Thread que falha morre sozinha | feito (`sched::kill_current`, `crash::fault`) | panic, `ud2` e `#PF` em `fetcher`/`wasmapp`: o compositor segue, Gerenciador mostra DEAD; no compositor, em `#DF` ou com IF=0 continua fatal |
| Canário de pilha | só como reserva quando não há guarda | fraco: um teste forçado corrompeu o heap antes da detecção |
| Recusa de framebuffer maior que os buffers | feito | "UNSUPPORTED SCREEN" (`docs/img/panic-oversize-*.png`) |
| Disco intocado se a leitura falhar | feito (`PERSIST`) | hash do disco idêntico com falha injetada |
| Gerador de números aleatórios real (DRBG ChaCha20 + pool SHA-256; HTTPS espera ou recusa se a entropia for fraca) | feito (`rng.rs`, `kitsune_core::entropy`) | 40 testes no core (RFC 8439, respostas conhecidas, estatística em 1 MiB), fuzz `entropy_api`, provas em QEMU em `design/entropy.md` §5 |

## 3. O que **não** é protegido

### 3.1 HTTPS (verificado, com ressalvas)
A pilha usa `embedded-tls` com um verificador próprio (`kernel/src/tlsv.rs`, lógica em
`kitsune_core::tlsverify`, sobre `rustls-webpki`): a cadeia do servidor é montada até uma
das 46 raízes embutidas (`kitsune_core/data/trust-store.bin`, SHA-256 de cada uma em
`trust-store.sha256`, regenerável com `tools/gen-trust-store.sh`), com assinaturas
(RSA PKCS#1/PSS, ECDSA P-256/P-384), validade, `basicConstraints`/`keyUsage`, EKU
`serverAuth`, restrições de nome e `pathLen`; o nome do site precisa constar no
`subjectAltName`; e o `CertificateVerify` do TLS 1.3 precisa conferir com a chave da folha.
Limites: 8 certificados, 16 KiB cada, handshake em até 30 s. O desenho completo está em
[`design/tls-browser.md`](design/tls-browser.md).

A barra de endereço diz o que aconteceu: **"Conexão segura"** só com cadeia válida,
"Certificado inválido" (vermelho) se o usuário abriu a página mesmo com erro, "Não seguro"
em `http://`. O tipo `Security` só chega a "seguro" por `Conn::Verified`, que o `fetcher`
devolve apenas depois da cadeia e da assinatura. Um erro de certificado bloqueia a página
com o motivo (expirado, nome não confere, cadeia não confiável, autoassinado, hora do
sistema incorreta...) e oferece **"Continuar mesmo assim (inseguro)"**: vale para aquele
host, nesta sessão, só na memória (8 hosts), nunca em disco.

**Hora.** A validade depende do relógio. O RTC é lido uma vez no boot e corrigido por SNTP
(`time.cloudflare.com`, `pool.ntp.org`, `time.google.com`, depois o gateway; resposta
validada: modo, leap, stratum, eco do *originate* com 16 bits aleatórios, ordem dos
timestamps, atraso, data entre 2024 e 2100). O deslocamento vale só para a checagem de
certificado. Sem resposta usa-se o RTC e a página avisa "Hora do sistema nao confirmada"
quando a checagem de data falha. **O SNTP não é autenticado**: quem controla a rede pode
mentir a hora e, com um certificado expirado que ainda tenha a chave, fazê-lo parecer
válido (o mesmo vale para quem controla o RTC e bloqueia o UDP/123).

O *client random* e a chave efêmera do handshake vêm do gerador do kernel (§3.7): um
DRBG ChaCha20 alimentado por `RDSEED`/`RDRAND`, pelo virtio-rng e por jitter de temporização.
O handshake **só começa** com pelo menos 128 bits de entropia creditados; sem eles espera até
5 s e depois **recusa** (falha de TLS e um aviso), em vez de usar um gerador fraco como antes.

**O que continua faltando:**
- **Revogação**: não existe CRL, OCSP nem grampeamento; um certificado revogado e ainda
  válido é aceito.
- **Pinning** e **HSTS**: nada impede um downgrade `http://` digitado pelo usuário, nem
  fixa a CA esperada de um site; Certificate Transparency não é consultada.
- A trust store é estática: raiz removida pelo Mozilla continua confiável até a próxima
  versão do Kitsune (atualização manual, `tools/gen-trust-store.sh`).
- Ed25519 em certificados de servidor não é suportado (o handshake falha com mensagem).
- A dependência `rsa` 0.9 tem o aviso RUSTSEC-2023-0071 (vazamento por tempo em operações
  de **chave privada**); aqui só verificamos assinaturas, não há chave privada RSA, e o
  aviso está ignorado de propósito em `deny.toml`.
- Em redes que reassinam o HTTPS (gateways corporativos, esta sandbox) toda página dá
  "cadeia nao confiavel": é o comportamento correto.

### 3.2 Sem isolamento
- Qualquer código do kernel lê e escreve toda a RAM e todo o MMIO (a memória física
  está mapeada em 1 TiB virtual, RW+NX).
- Não há SMEP/SMAP, ring 3, paginação por processo, nem syscalls.
- "Processos" são linhas de uma tabela, não espaços de endereçamento. O gerenciador
  de tarefas mostra CPU real por thread, mas matar um "processo" só fecha a janela.
- O caminho de evolução está no [ADR de isolamento](audit/adr-isolamento.md): primeiro
  endurecer o kernel, depois tornar o WebAssembly a fronteira (já existe, com
  limites), e adiar o ring 3 até haver um gatilho concreto.

### 3.3 Pilhas e memória
- As pilhas de `fetcher` e `wasmapp` (128 KiB cada) vêm do heap, **com página de
  guarda** (a página de nível 1 é desmapeada; o heap em `.bss` usa páginas de 4 KiB
  em BIOS e UEFI). Uso medido: 9–13 KiB. Um estouro dentro de uma interrupção que roda
  na própria pilha da thread (IF=0) continua fatal, com a mensagem certa.
- O heap é um bloco único de 64 MiB sem cota por consumidor. Esgotá-lo vira panic
  (agora visível), não corrupção.
- Uma thread secundária que falha **morre sozinha**, mas não é reiniciada e não libera
  nada: sua pilha, suas alocações e a `Store` do `wasmi` ficam; um lock que ela
  segurava com interrupções ligadas fica preso. O navegador e o app WASM ficam
  inutilizáveis até o reboot (a interface mostra o motivo).

### 3.4 Rede
- O IP, o gateway e os DNS do navegador vêm do lease DHCP (com fallback estático
  `10.0.2.15/24` do SLIRP quando não há servidor). O lease **é renovado** (RENEW unicast em
  T1, REBIND em T2) e o endereço é **removido** ao expirar. O DHCP não é autenticado: quem
  responder primeiro define o gateway e os DNS, e um servidor falso que responde a um RENEW
  com outra configuração reconfigura a interface. O DNS tenta todos os servidores do lease, mas
  também não é autenticado nem tem DNSSEC; o id da consulta e a porta de origem vêm do gerador do kernel (§3.7), então são tão imprevisíveis quanto ele.
- A NIC tem **um dono** (`netd`, no tipo) e o compositor não a toca. O virtio-net só faz DMA
  nos próprios buffers estáticos (RX/TX de 2 KiB) e valida cada elemento do anel `used` (id
  fora da fila, índice impossível, quadro curto ou longo) antes de usar; sem IOMMU, um
  dispositivo virtio malicioso ainda lê e escreve a memória física.
- Drivers: virtio-net (QEMU, VMs) e NE2000 (ISA, QEMU); não há `e1000`/`rtl8139` nem driver para
  a NIC de um PC comum.

### 3.5 Dados
- O OJFS guarda dono, grupo e modo em cada inode e o VFS sabe impô-los (`storage::secured`, testado
  no host e com fuzz de `security::*`), e **o kernel o usa**: há contas (`/etc/accounts`, senha PBKDF2), tela de login e bloqueio, e os
  arquivos do usuário passam por `Secured`. Programas ELF e os apps WebAssembly v2 já rodam em anel 3, um processo por app ([`design/processos.md`](design/processos.md)); o desktop, os apps v1 e os serviços ainda não. Mesmo assim o desktop e os serviços do
  sistema rodam no anel 0 com acesso privilegiado, então as permissões protegem um usuário de outro dentro dos apps de
  arquivos, não de código malicioso no kernel. Não há criptografia do disco. O disco dedicado é tratado como confiável
  depois de validado. O plano e a ordem estão em [`design/usuarios-seguranca.md`](design/usuarios-seguranca.md).

### 3.6 Apps: dados persistentes e rede (W18)
Os apps passaram a ter **superfície persistente** (o disco) e **superfície de rede real**.
- **Disco.** `/apps/<id>.wasm`, `/data/<id>` e `/home` vivem no volume OJFS v3 (antes eram RAM).
  O app nunca fala com o volume: o `Sandbox` traduz a raiz dele (`/data/<id>` ou `/home`), valida
  cada caminho por componentes, mede a cota (`disk_kib`, 256 B por entrada + tamanho; o que já
  está em disco conta depois de um reboot) e limita descritores; o `VolumeFs` repete a defesa:
  só `/apps`, `/data` e `/home` existem para um app, as pastas-raiz não saem, e o arquivo
  tem teto de 64 MiB. Um app cheio de dados pode encher o **volume** (não só a sua cota) se
  tiver `fs=home`, pois a cota de `home` conta só o que ele escreveu naquela execução: o
  usuário vê "Disco cheio" no Arquivos e o `fsck` continua limpo (testado com um volume de 1 MiB).
  Os dados de um app removido ficam em `/data/<id>` até o usuário apagá-los pelo Arquivos.
  Cada chamada de arquivo é uma seção crítica do `YieldMutex` do volume, nunca atravessa a
  execução do guest, e uma chamada aninhada ou um dono morto viram erro, não travamento.
- **Rede.** `net_http_get` agora transporta de verdade (thread `fetcher`, a dona da NIC).
  Regras, todas testadas no host e provadas em QEMU contra um servidor falso:
  1. precisa de `net=http`/`tcp`; sem isso `ERR_PERM` sem sequer analisar a URL;
  2. o filtro de destinos (`appnet`) recusa `localhost`, nomes de uma só etiqueta, `.local`/
     `.internal`/`.lan`, IPv6, IPv4 privado/loopback/reservado e IPs disfarçados; o gateway
     do QEMU e a LAN ficam fora de alcance;
  3. `net_hosts` (opcional) restringe a uma lista exata ou com curinga de subdomínios;
  4. a regra vale para **cada salto de redirecionamento** (um `Location` para IP privado, outro
     host ou `https -> http` é recusado) e para o **endereço resolvido** (um nome público que
     aponta para loopback ou para a LAN é barrado depois do DNS: `Net::set_public_only`);
  5. **TLS com verificação completa** (§3.1): cadeia, nome, `CertificateVerify`; para um app não
     existe o "continuar mesmo assim", então um certificado autoassinado, expirado ou de outro
     nome é `ERR_NET` e nada do corpo chega ao guest; `http://` é aceito (o manifesto pediu
     `net=http`), sem rebaixar `https` por redirecionamento;
  6. só o corpo de uma resposta 2xx, decodificado e cortado em 256 KiB; no máximo 1 pedido por
     segundo por app e um pedido por vez no sistema; tempo limite de 8 s (20 s em https).
  Limite conhecido: o pedido é síncrono e há uma só thread `appd`, então um app esperando a rede
  atrasa os **outros apps** (não o compositor) até o prazo; é negação de serviço entre apps
  (todos rodam em ring 0 e o app já pode gastar o combustível de qualquer forma), não fuga.
  O conteúdo de uma resposta é dado não confiável: o decodificador gzip é limitado (1 MiB) e o
  guest recebe bytes, nunca algo que o kernel interprete.

### 3.7 Números aleatórios (W21)
Tudo o que precisa ser imprevisível (*client random* e chave efêmera do TLS, ISN do TCP, `xid` do
DHCP, id e porta do DNS, nonce do SNTP, porta local, `random_get` dos apps WASM) sai de
**uma** função, `crate::rng::fill` (`kernel/src/rng.rs`), um DRBG ChaCha20 de apagamento rápido
de chave sobre um pool SHA-256. Desenho completo, fontes e provas em
[`design/entropy.md`](design/entropy.md). O que importa aqui:

| Nota | Quando | O que se pode dizer |
|---|---|---|
| **Strong** | `RDSEED`, `RDRAND` ou virtio-rng deram >= 128 bits de crédito à chave | tão forte quanto o gerador de hardware (ou o hospedeiro, no virtio-rng) e o ChaCha20; confiança em caixa-preta mitigada porque nada é usado cru |
| **Mixed** | sem hardware, >= 128 bits de crédito de temporização (timer, teclado, mouse, NIC, jitter de CPU) | bom contra um atacante que não vê os relógios da máquina; **sem garantia** contra quem vê |
| **Weak** | menos de 128 bits | o DRBG responde (ids sem segredo), mas o HTTPS espera até 5 s e recusa |

- **Crédito conservador:** amostra de interrupção vale **no máximo 0,5 bit** e só se passar nos
  testes de "preso / variação mínima / repetição" sobre as três primeiras diferenças dos
  timestamps; jitter de CPU vale 0,1 bit; RTC e TSC de boot entram no hash com crédito **zero**.
  O crédito é uma afirmação, não uma medição.
- **Numa VM totalmente determinística sem virtio-rng e sem `RDRAND` a qualidade é, no máximo,
  "Mixed", e pode ser zero de verdade** (o estimador só enxerga timestamps: um relógio
  perfeitamente regular é rejeitado e a nota fica Weak, mas um relógio determinístico com
  variação sintética passaria). Por isso timing sozinho **nunca** chega a Strong. O caminho
  honesto é dar entropia ao guest: os scripts e o `run.ps1` já passam `-device virtio-rng-pci`
  por padrão (`QEMU_RNG=none` / `-NoRng` desligam, para testar o caminho de jitter).
- **No QEMU com WHPX (o caso do dono):** o `RDRAND` fica escondido do guest; com virtio-rng a
  nota é Strong, sem ele é Mixed depois de ~1-2 s de ticks do timer e o HTTPS funciona, com uma
  linha INFO na serial (`RNG: pool seeded from timing jitter (N bits credited)`), sem toast.
- **Confiança:** virtio-rng é a entropia do hospedeiro; o hipervisor já controla toda a memória
  do guest, então isto não amplia o que ele pode fazer. `RDRAND`/`RDSEED` são caixas-pretas: ficam
  atrás do pool e do DRBG, valores sabidamente ruins (0, todos os bits, `0xFFFF_FFFF`, bloco constante)
  desligam a fonte depois de 3 blocos ruins seguidos.
- **Interrupções:** as ISRs só chamam `rng::sample` (um `rdtsc` e dois acessos atômicos a um vetor
  estático; sem alocação, sem lock, sem log). O estimador e o SHA-256 rodam em contexto de thread.
- **Limites conhecidos:** sem semente persistente entre boots; estado protegido por interrupções
  desligadas (um só núcleo); o apagamento de memória é o que Rust seguro permite (`fill(0)` +
  `black_box`).

## 4. Cenários de ataque e resultado hoje

| Cenário | O que acontecia antes da auditoria | Hoje |
|---|---|---|
| Servidor responde `Transfer-Encoding: chunked` com tamanho gigante | `panic` no kernel (máquina parada) | tratado, teste de regressão |
| Página com ~150 `<div>` aninhados | estouro da pilha de 80 KiB → triple fault | profundidade limitada a 40, teste e fuzz |
| CSS `color:#é1` | `panic` por fatiar no meio de um caractere | rejeitado, teste e fuzz |
| CSS `margin:2147483647` | overflow em release → coordenadas negativas | limitado a 4096 px |
| URL com porta `4294967376` | conecta na porta 80 | saturada e rejeitada |
| Redirect `Location` com CRLF ou `https`→`http` | aceito | rejeitado / bloqueado |
| Disco com `parent` em ciclo | recursão infinita, estouro de pilha | corrigido |
| Disco com `size = 0xFFFF` | leitura além do registro e `panic` | limitado ao máximo |
| Disco ilegível por erro transitório | **formatado e sobrescrito** | intocado |
| Guest WASM em laço infinito | CPU presa para sempre, janela fechada não o parava | encerrado por *fuel*, liberado |
| Guest WASM com `memory.grow` sem fim | 51% do heap | limitado a 24 MiB |
| `random_get(0x7fffffff)` / `fd_write` com 2³¹ iovecs | trabalho ilimitado | `EINVAL` |
| MITM em HTTPS | possível | **bloqueado** pela verificação de cadeia/nome/assinatura (§3.1); ainda possível com uma raiz da trust store comprometida, sem revogação, ou mentindo a hora por SNTP |
| Servidor malicioso faz resposta de vários MiB | OOM mudo no `http_get` | truncado em 1 MiB (4 MiB já descompactado), avisado na página |
| App tenta ler/escrever `/etc/kitsune.conf`, `/var/log`, `/.trash` ou os arquivos do usuário | (apps sem disco real) | `ERR_PERM` no `VolumeFs`, mesmo que o `Sandbox` falhasse (testes e fuzz sobre o volume) |
| App pede uma URL que redireciona para `http://10.0.2.2/` ou para um nome público que resolve para `127.0.0.1` | — (`net_http_get` não transportava) | recusado no salto / depois do DNS; provado em QEMU (`w18-net.sh`) |
| Sem `RDRAND` (QEMU com WHPX): *client random* e chave efêmera do TLS | de um misturador de 64 bits (TSC e ticks); toast `RNG: weak fallback` a cada boot | DRBG ChaCha20 semeado por virtio-rng ou, sem ele, por jitter (>= 128 bits creditados antes do handshake, senão recusa); provado em QEMU (`design/entropy.md` §5) |
| ISN do TCP | zero (`smoltcp` com semente 0) | semente do gerador do kernel |
| App contra um servidor HTTPS autoassinado | — | `tls: certificate check FAILED`, o app recebe `ERR_NET` e nenhum byte |

## 5. Como reproduzir as provas

- Testes: `cargo test-core` (2373 testes; regressões dos achados em `kitsune_core`, 40 deles do gerador de entropia).
- Fuzz: [`TESTING.md`](TESTING.md#2-fuzzing) — as entradas mínimas dos crashes estão em
  `fuzz/regressions/`.
- Falhas visíveis e IST: [`TESTING.md`](TESTING.md#provando-falhas-padrão-usado-na-auditoria).
- Limites do WASM: guests hostis foram gerados com `wat` num gancho temporário de
  build (laço infinito, `memory.grow` em laço, `proc_exit`, `random_get` gigante) e
  o log da serial mostra o app encerrado com o desktop respondendo.
- Relatório completo e rastreabilidade de cada achado: [`audit/RELATORIO.md`](audit/RELATORIO.md).

Para relatar uma vulnerabilidade, veja [`../SECURITY.md`](../SECURITY.md).
